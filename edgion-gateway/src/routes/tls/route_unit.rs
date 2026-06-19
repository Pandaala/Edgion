use std::sync::Arc;

use super::{cap_tls_connect_retries, DEFAULT_TLS_CONNECT_RETRIES, MAX_TLS_CONNECT_RETRIES};
use crate::lb::BackendSelector;
use crate::routes::common::{is_core_service_backend_ref, resolve_stream_plugin_store_key, resolve_tcp_keepalive};
use edgion_resources::resources::common::TcpKeepaliveConfig;
use edgion_resources::resources::tls_route::TLSBackendRef;
use edgion_resources::resources::TLSRoute;
use edgion_resources::ResourceMeta;

/// Gateway-side compiled companion for a TLSRoute.
///
/// Holds the route schema plus the compiled runtime that cannot live in
/// `src/types` (BackendSelector, proxy-protocol version, connect-retry budget,
/// keepalive config, stream-plugin store key). Built ONCE per route and shared
/// (Arc-cloned) across every SNI hostname bucket the route serves, so the WRR
/// selector cursor and per-rule state are not split per hostname.
///
/// Per-protocol note: TLS routes fan out across SNI hostnames but a single
/// route shares one rule set, so the compiled runtime is built from the first
/// rule (matching the data-plane read path `rules.first()`).
pub struct TlsRouteUnit {
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
    /// builds the WRR selector from the first rule's backend refs. None of this
    /// state lives on the schema anymore.
    pub fn from_route(route: Arc<TLSRoute>) -> Self {
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

/// Build a WRR selector over the first rule's core-service backend refs.
fn build_backend_selector(route: &TLSRoute) -> Arc<BackendSelector<TLSBackendRef>> {
    let selector = BackendSelector::new();
    if let Some(rule) = route.spec.rules.as_ref().and_then(|r| r.first()) {
        if let Some(backend_refs) = &rule.backend_refs {
            let (backends, weights): (Vec<_>, Vec<_>) = backend_refs
                .iter()
                .filter(|br| is_core_service_backend_ref(br.group.as_deref(), br.kind.as_deref()))
                .map(|br| (br.clone(), br.weight))
                .unzip();
            if !backends.is_empty() {
                selector.init(backends, weights);
            }
        }
    }
    Arc::new(selector)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::ParentReference;
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
                        extension_info: Default::default(),
                        ref_denied: None,
                    }]),
                }]),
                resolved_listeners: None,
                effective_hostnames: None,
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
}
