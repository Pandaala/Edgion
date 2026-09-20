use crate::routes::common::filter_accepted_parent_refs;
use crate::runtime::matching::route::ResolvedParentRef;
use crate::runtime::store::listener_gateway_info::format_gateway_key;
use edgion_algo::matcher::radix_hostname::RadixHostnamePatternRef;
use std::sync::Arc;

use super::{cap_tls_connect_retries, DEFAULT_TLS_CONNECT_RETRIES, MAX_TLS_CONNECT_RETRIES};
use crate::lb::BackendSelector;
use crate::routes::common::{resolve_stream_plugin_store_key, resolve_tcp_keepalive};
use edgion_resources::resources::common::TcpKeepaliveConfig;
use edgion_resources::resources::tls_route::TLSBackendRef;
use edgion_resources::resources::TLSRoute;
use edgion_resources::ResourceMeta;

pub(crate) struct AcceptedTlsAttachment {
    pub schema_index: usize,
    pub parent: ResolvedParentRef,
}

/// Gateway-side compiled companion for a TLSRoute.
///
/// Holds the route schema plus the gateway-side compiled runtime (BackendSelector,
/// proxy-protocol version, connect-retry budget, keepalive config,
/// stream-plugin store key). Built ONCE per route and shared (Arc-cloned) across
/// every SNI hostname bucket the route serves, so the WRR selector cursor and
/// per-rule state are not split per hostname.
///
/// Per-protocol note: TLS routes fan out across SNI hostnames but a single
/// route shares one rule set, so the compiled runtime is built from the first
/// rule (matching the data-plane read path `rules.first()`).
pub struct TlsRouteUnit {
    /// Each effective hostname list stays paired with its exact logical owner.
    pub(crate) accepted_attachments: Vec<AcceptedTlsAttachment>,
    pub route: Arc<TLSRoute>,
    /// WRR selector over the first rule's backends (one per route, shared
    /// across all SNI hostnames that map to this route).
    pub backend_finder: Arc<BackendSelector<TLSBackendRef>>,
    /// Store key for dynamic stream-plugin lookup (from route annotations).
    pub stream_plugin_store_key: Option<String>,
    /// Proxy Protocol version to send upstream (None = disabled, Some(2) = PP2).
    pub proxy_protocol_version: Option<u8>,
    /// Max upstream connect attempts (1 = no retry).
    pub max_connect_retries: u32,
    /// Resolved upstream TCP keepalive config (from route annotations).
    pub tcp_keepalive: Option<TcpKeepaliveConfig>,
    /// Operator log labels resolved once from the route metadata (annotations /
    /// labels) at build time, attached to `rsc.labels` in the TLS access log.
    /// Resolving here keeps the per-connection match path off the metadata scan.
    pub log_labels: Option<crate::LogLabels>,
}

impl TlsRouteUnit {
    /// Build a unit from a cached route.
    ///
    /// Resolves the proxy-protocol version, stream-plugin store key, connect
    /// retry budget and keepalive config from the route's metadata/annotations
    /// (single source of truth shared with the HTTP/TCP conf-handlers) and
    /// builds the WRR selector from the first rule's backend refs.
    pub fn from_route(route: Arc<TLSRoute>) -> Self {
        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let accepted = filter_accepted_parent_refs(
            route.spec.parent_refs.as_deref(),
            route.status.as_ref().map(|status| status.parents.as_slice()),
            Some(namespace),
            route.spec.resolved_status_controller.as_deref(),
        )
        .unwrap_or_default();
        let mut accepted_attachments: Vec<AcceptedTlsAttachment> = Vec::new();
        if route.spec.validate_hostnames().is_ok() {
            // Preserve authored parent order: admission selects the first owner.
            for parent in &accepted {
                for (i, attachment) in route
                    .spec
                    .resolved_listeners
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    let port = attachment
                        .listener_key
                        .0
                        .split_once('/')
                        .and_then(|(addr, _)| addr.parse::<std::net::SocketAddr>().ok())
                        .map(|addr| i32::from(addr.port()));
                    if attachment.listener_key.protocol() != "TLS"
                        || attachment.hostnames.is_empty()
                        || attachment
                            .hostnames
                            .iter()
                            .any(|hostname| RadixHostnamePatternRef::parse(hostname).is_err())
                        || parent.namespace.as_deref().unwrap_or(namespace) != attachment.gateway_namespace
                        || parent.name != attachment.gateway_name
                        || parent
                            .section_name
                            .as_deref()
                            .is_some_and(|name| name != attachment.listener_name)
                        || parent.port.is_some_and(|required| Some(required) != port)
                        || accepted_attachments.iter().any(|accepted| accepted.schema_index == i)
                    {
                        continue;
                    }
                    accepted_attachments.push(AcceptedTlsAttachment {
                        schema_index: i,
                        parent: ResolvedParentRef {
                            port: parent.port,
                            gateway_key: format_gateway_key(&attachment.gateway_namespace, &attachment.gateway_name),
                            parent_ns: attachment.gateway_namespace.clone(),
                            section_name: Some(attachment.listener_name.clone()),
                        },
                    });
                }
            }
        }

        let route_key = route.key_name();
        let annotations = route.metadata.annotations.as_ref();

        let proxy_protocol_version = annotations
            .and_then(|a| a.get(edgion_resources::constants::annotations::edgion::PROXY_PROTOCOL))
            .and_then(|v| match v.trim() {
                "v2" => Some(2u8),
                _ => None,
            });

        let stream_plugin_store_key = resolve_stream_plugin_store_key(&route.metadata);

        let configured_max_connect_retries = annotations
            .and_then(|a| a.get(edgion_resources::constants::annotations::edgion::MAX_CONNECT_RETRIES))
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map(|v| v.max(1))
            .unwrap_or(DEFAULT_TLS_CONNECT_RETRIES);
        let max_connect_retries = cap_tls_connect_retries(configured_max_connect_retries);
        if configured_max_connect_retries > MAX_TLS_CONNECT_RETRIES {
            tracing::warn!(
                route = %route_key,
                configured_max_connect_retries,
                max_connect_retries,
                "TLSRoute max_connect_retries capped"
            );
        }

        let tcp_keepalive = resolve_tcp_keepalive(annotations, &route_key);
        let log_labels = crate::extract_log_labels(&route.metadata);
        let backend_finder = build_backend_selector(&route);

        TlsRouteUnit {
            route,
            accepted_attachments,
            backend_finder,
            stream_plugin_store_key,
            proxy_protocol_version,
            max_connect_retries,
            tcp_keepalive,
            log_labels,
        }
    }

    /// Delegate to the inner route's resource key (e.g. `"default/my-route"`).
    pub fn key_name(&self) -> String {
        self.route.key_name()
    }
}

/// Build a WRR selector over the first rule's backend refs.
///
/// Denied and non-Service refs keep their declared weight. Fail-close is
/// the post-select dial gate, not selector construction.
fn build_backend_selector(route: &TLSRoute) -> Arc<BackendSelector<TLSBackendRef>> {
    let selector = BackendSelector::new();
    if let Some(rule) = route.spec.rules.as_ref().and_then(|r| r.first()) {
        if let Some(backend_refs) = &rule.backend_refs {
            let (backends, weights): (Vec<_>, Vec<_>) = backend_refs.iter().map(|br| (br.clone(), br.weight)).unzip();
            if !backends.is_empty() {
                selector.init(backends, weights);
            }
        }
    }
    Arc::new(selector)
}

#[cfg(test)]
mod tests {
    #[test]
    fn malformed_effective_hostname_rejects_the_entire_attachment() {
        let mut route: TLSRoute = serde_json::from_value(serde_json::json!({
            "metadata": {"namespace": "apps", "name": "invalid-effective"},
            "spec": {"hostnames": ["*.example.com"], "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{"name": "gw"}],
                "resolvedListeners": [
                    {"listenerKey": "0.0.0.0:443/TLS", "gatewayNamespace": "apps",
                        "gatewayName": "gw", "listenerName": "tls",
                        "hostnames": ["valid.example.com", "*.EXAMPLE.com"]}
                ]}
        }))
        .unwrap();
        assert!(TlsRouteUnit::from_route(Arc::new(route.clone()))
            .accepted_attachments
            .is_empty());
        route.spec.resolved_listeners.as_mut().unwrap()[0].hostnames.pop();
        assert_eq!(TlsRouteUnit::from_route(Arc::new(route)).accepted_attachments.len(), 1);
    }

    #[test]
    fn exact_attachment_parents_preserve_authored_owner_order() {
        let route = serde_json::from_value(serde_json::json!({
            "metadata": {"namespace": "apps", "name": "ordered"},
            "spec": {"hostnames": ["shared.example.com"],
                "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{"name": "z"}, {"name": "a"}],
                "resolvedListeners": [
                    {"listenerKey": "0.0.0.0:443/TLS", "gatewayNamespace": "apps",
                        "gatewayName": "a", "listenerName": "tls", "hostnames": ["shared.example.com"]},
                    {"listenerKey": "0.0.0.0:443/TLS", "gatewayNamespace": "apps",
                        "gatewayName": "z", "listenerName": "tls", "hostnames": ["shared.example.com"]}
                ]}
        }))
        .unwrap();
        let unit = super::TlsRouteUnit::from_route(std::sync::Arc::new(route));
        let key = edgion_resources::ListenerKey::new("0.0.0.0:443".parse().unwrap(), "TLS");
        let parents: Vec<_> = unit
            .accepted_attachments
            .iter()
            .filter(|accepted| {
                unit.route.spec.resolved_listeners.as_ref().unwrap()[accepted.schema_index].listener_key == key
            })
            .map(|accepted| &accepted.parent)
            .collect();
        assert_eq!(
            parents
                .iter()
                .map(|parent| parent.gateway_key.as_str())
                .collect::<Vec<_>>(),
            ["apps/z", "apps/a"]
        );
        assert!(parents
            .iter()
            .all(|parent| parent.section_name.as_deref() == Some("tls")));
    }

    use super::*;
    use edgion_resources::resources::common::{ParentReference, RefDenied};
    use edgion_resources::resources::tls_route::{TLSBackendRef, TLSRouteRule, TLSRouteSpec};

    fn make_route(annotations: Option<std::collections::BTreeMap<String, String>>) -> Arc<TLSRoute> {
        Arc::new(TLSRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("r".to_string()),
                annotations,
                ..Default::default()
            },
            spec: TLSRouteSpec {
                resolved_status_controller: Some("edgion.io/gateway-controller".into()),
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some("default".to_string()),
                    name: "gw".to_string(),
                    section_name: None,
                    port: None,
                }]),
                hostnames: Some(vec!["test.com".to_string()]),
                rules: Some(vec![TLSRouteRule {
                    backend_refs: Some(vec![TLSBackendRef {
                        name: "svc".to_string(),
                        namespace: Some("default".to_string()),
                        port: Some(8080),
                        weight: Some(1),
                        group: None,
                        kind: None,
                        ref_denied: None,
                    }]),
                }]),
                resolved_listeners: None,
            },
            status: None,
        })
    }

    fn annotated(pairs: &[(&str, &str)]) -> Arc<TLSRoute> {
        let mut a = std::collections::BTreeMap::new();
        for (k, v) in pairs {
            a.insert(k.to_string(), v.to_string());
        }
        make_route(Some(a))
    }

    #[test]
    fn test_proxy_protocol_v2_resolved_on_unit() {
        let unit = TlsRouteUnit::from_route(annotated(&[("edgion.io/proxy-protocol", "v2")]));
        assert_eq!(unit.proxy_protocol_version, Some(2));
        assert!(unit.stream_plugin_store_key.is_none());
    }

    #[test]
    fn test_proxy_protocol_invalid_resolves_none() {
        let unit = TlsRouteUnit::from_route(annotated(&[("edgion.io/proxy-protocol", "v1")]));
        assert_eq!(unit.proxy_protocol_version, None);
    }

    #[test]
    fn test_stream_plugin_store_key_full_path() {
        let unit = TlsRouteUnit::from_route(annotated(&[(
            "edgion.io/edgion-stream-plugins",
            "sandbox/sandbox-stream-plugins",
        )]));
        assert_eq!(
            unit.stream_plugin_store_key,
            Some("sandbox/sandbox-stream-plugins".to_string())
        );
    }

    #[test]
    fn test_combined_annotations() {
        let unit = TlsRouteUnit::from_route(annotated(&[
            ("edgion.io/proxy-protocol", "v2"),
            ("edgion.io/edgion-stream-plugins", "ns/plugins"),
        ]));
        assert_eq!(unit.proxy_protocol_version, Some(2));
        assert_eq!(unit.stream_plugin_store_key, Some("ns/plugins".to_string()));
    }

    #[test]
    fn test_no_annotations_defaults() {
        let unit = TlsRouteUnit::from_route(make_route(None));
        assert_eq!(unit.proxy_protocol_version, None);
        assert!(unit.stream_plugin_store_key.is_none());
        assert_eq!(unit.max_connect_retries, DEFAULT_TLS_CONNECT_RETRIES);
        assert!(unit.tcp_keepalive.is_none());
    }

    #[test]
    fn test_keepalive_annotation_resolved_on_unit() {
        let unit = TlsRouteUnit::from_route(annotated(&[
            ("edgion.io/tcp-keepalive-time", "120"),
            ("edgion.io/tcp-keepalive-interval", "20"),
            ("edgion.io/tcp-keepalive-probes", "4"),
        ]));
        let ka = unit.tcp_keepalive.as_ref().expect("keepalive enabled");
        assert_eq!(ka.idle, std::time::Duration::from_secs(120));
        assert_eq!(ka.interval, std::time::Duration::from_secs(20));
        assert_eq!(ka.count, 4);
    }

    #[test]
    fn test_backend_finder_built_from_first_rule() {
        let unit = TlsRouteUnit::from_route(make_route(None));
        assert!(unit.backend_finder.select().is_ok());
    }

    #[test]
    fn denied_core_service_ref_remains_selectable() {
        let mut route = make_route(None);
        {
            let route = Arc::make_mut(&mut route);
            let backend = route
                .spec
                .rules
                .as_mut()
                .and_then(|rules| rules.first_mut())
                .and_then(|rule| rule.backend_refs.as_mut())
                .and_then(|refs| refs.first_mut())
                .expect("fixture has one backend ref");
            backend.ref_denied = Some(RefDenied {
                target_namespace: "other".to_string(),
                target_name: "svc".to_string(),
                reason: Some("NoMatchingReferenceGrant".to_string()),
            });
        }
        let unit = TlsRouteUnit::from_route(route);
        let selected = unit
            .backend_finder
            .select()
            .expect("denied core Service stays selectable");
        assert!(selected.ref_denied.is_some());
    }

    fn tls_backend(name: &str, group: Option<&str>, kind: Option<&str>, denied: bool) -> TLSBackendRef {
        TLSBackendRef {
            name: name.to_string(),
            namespace: Some("default".to_string()),
            port: Some(8080),
            weight: Some(1),
            group: group.map(str::to_string),
            kind: kind.map(str::to_string),
            ref_denied: denied.then(|| RefDenied {
                target_namespace: "other".to_string(),
                target_name: name.to_string(),
                reason: Some("NoMatchingReferenceGrant".to_string()),
            }),
        }
    }

    fn mixed_weight_route() -> Arc<TLSRoute> {
        let mut route = make_route(None);
        {
            let route = Arc::make_mut(&mut route);
            let refs = route
                .spec
                .rules
                .as_mut()
                .and_then(|rules| rules.first_mut())
                .and_then(|rule| rule.backend_refs.as_mut())
                .expect("fixture has backend refs");
            *refs = vec![
                tls_backend("allowed", None, None, false),
                tls_backend("denied", None, None, true),
                tls_backend("invalid", Some("apps"), Some("Deployment"), false),
            ];
        }
        route
    }

    #[test]
    fn mixed_denied_and_unsupported_keep_declared_weight() {
        use crate::routes::common::{classify_l4_selected_backend, L4SelectedBackend};
        let unit = TlsRouteUnit::from_route(mixed_weight_route());
        let mut allowed = 0;
        let mut denied = 0;
        let mut invalid = 0;
        for _ in 0..3 {
            let selected = unit.backend_finder.select().expect("three equal-weight slots");
            let outcome = classify_l4_selected_backend(
                selected.ref_denied.as_ref(),
                selected.group.as_deref(),
                selected.kind.as_deref(),
            );
            match selected.name.as_str() {
                "allowed" => {
                    allowed += 1;
                    assert_eq!(outcome, L4SelectedBackend::Proceed);
                }
                "denied" => {
                    denied += 1;
                    assert_eq!(outcome, L4SelectedBackend::Denied);
                }
                "invalid" => {
                    invalid += 1;
                    assert_eq!(outcome, L4SelectedBackend::UnsupportedKind);
                }
                other => panic!("unexpected backend {other}"),
            }
        }
        assert_eq!((allowed, denied, invalid), (1, 1, 1));
    }
}
