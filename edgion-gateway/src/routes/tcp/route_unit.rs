use crate::routes::common::{filter_accepted_parent_refs, resolve_parent_refs};
use crate::runtime::matching::route::ResolvedParentRef;
use std::sync::Arc;

use crate::lb::BackendSelector;
use crate::routes::common::{resolve_stream_plugin_store_key, resolve_tcp_keepalive};
use edgion_resources::resources::common::TcpKeepaliveConfig;
use edgion_resources::resources::tcp_route::TCPBackendRef;
use edgion_resources::resources::TCPRoute;
use edgion_resources::ResourceMeta;

/// Gateway-side compiled companion for a TCPRoute.
///
/// Holds the route schema plus the compiled runtime that cannot live in
/// `src/types` (BackendSelector, keepalive config, stream-plugin store key).
/// Built once per route from the cached `Arc<TCPRoute>`; the inner `route`
/// field keeps the full schema accessible for the data plane.
///
/// Per-protocol note: TCP has no hostname fan-out — one unit per route
/// suffices. `backend_finder` is built for the first rule only, matching the
/// data-plane read path (`rules.first()`).
pub struct TcpRouteUnit {
    /// Accepted parent identities resolved once for logical Gateway admission.
    pub resolved_parent_refs: Vec<ResolvedParentRef>,
    pub route: Arc<TCPRoute>,
    /// WRR selector over the first rule's backends.
    pub backend_finder: Arc<BackendSelector<TCPBackendRef>>,
    /// Store key for dynamic stream-plugin lookup (from route annotations).
    pub stream_plugin_store_key: Option<String>,
    /// Resolved TCP keepalive config (from route annotations).
    pub tcp_keepalive: Option<TcpKeepaliveConfig>,
}

impl TcpRouteUnit {
    /// Build a unit from a cached route.
    ///
    /// Resolves the stream-plugin store key and keepalive config from the
    /// route's metadata/annotations (single source of truth shared with the
    /// HTTP/TLS conf-handlers) and builds the WRR selector from the first
    /// rule's backend refs. None of this state lives on the schema.
    pub fn from_route(route: Arc<TCPRoute>) -> Self {
        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let accepted = filter_accepted_parent_refs(
            route.spec.parent_refs.as_deref(),
            route.status.as_ref().map(|status| status.parents.as_slice()),
            Some(namespace),
            route.spec.resolved_status_controller.as_deref(),
        )
        .unwrap_or_default();
        let resolved_parent_refs = resolve_parent_refs(&accepted, namespace);

        let route_key = route.key_name();
        let stream_plugin_store_key = resolve_stream_plugin_store_key(&route.metadata);
        let tcp_keepalive = resolve_tcp_keepalive(route.metadata.annotations.as_ref(), &route_key);
        let backend_finder = build_backend_selector(&route);
        TcpRouteUnit {
            route,
            resolved_parent_refs,
            backend_finder,
            stream_plugin_store_key,
            tcp_keepalive,
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
fn build_backend_selector(route: &TCPRoute) -> Arc<BackendSelector<TCPBackendRef>> {
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
    use super::*;
    use edgion_resources::resources::common::{ParentReference, RefDenied};
    use edgion_resources::resources::tcp_route::{TCPBackendRef, TCPRouteRule, TCPRouteSpec};

    fn make_route(annotations: Option<std::collections::BTreeMap<String, String>>) -> Arc<TCPRoute> {
        Arc::new(TCPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("r".to_string()),
                annotations,
                ..Default::default()
            },
            spec: TCPRouteSpec {
                resolved_status_controller: Some("edgion.io/gateway-controller".into()),
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some("default".to_string()),
                    name: "gw".to_string(),
                    section_name: Some("tcp".to_string()),
                    port: None,
                }]),
                rules: Some(vec![TCPRouteRule {
                    backend_refs: Some(vec![TCPBackendRef {
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

    #[test]
    fn test_keepalive_annotation_resolved_on_unit() {
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("edgion.io/tcp-keepalive-time".to_string(), "90".to_string());
        let unit = TcpRouteUnit::from_route(make_route(Some(annotations)));
        let ka = unit.tcp_keepalive.as_ref().expect("keepalive enabled");
        assert_eq!(ka.idle, std::time::Duration::from_secs(90));
        assert_eq!(ka.interval, std::time::Duration::from_secs(75)); // default
        assert_eq!(ka.count, 9); // default
    }

    #[test]
    fn test_no_keepalive_annotation_leaves_unit_field_none() {
        let unit = TcpRouteUnit::from_route(make_route(None));
        assert!(unit.tcp_keepalive.is_none());
    }

    #[test]
    fn test_backend_finder_built_from_first_rule() {
        let unit = TcpRouteUnit::from_route(make_route(None));
        // A single core-service backend ref is selectable.
        assert!(unit.backend_finder.select().is_ok());
    }

    #[test]
    fn denied_core_service_ref_remains_selectable() {
        // Denied refs keep their declared weight; fail-close is the dial
        // gate, not selector construction.
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
        let unit = TcpRouteUnit::from_route(route);
        let selected = unit
            .backend_finder
            .select()
            .expect("denied core Service stays selectable");
        assert!(selected.ref_denied.is_some());
    }

    fn tcp_backend(name: &str, group: Option<&str>, kind: Option<&str>, denied: bool) -> TCPBackendRef {
        TCPBackendRef {
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

    fn mixed_weight_route() -> Arc<TCPRoute> {
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
                tcp_backend("allowed", None, None, false),
                tcp_backend("denied", None, None, true),
                tcp_backend("invalid", Some("apps"), Some("Deployment"), false),
            ];
        }
        route
    }

    #[test]
    fn mixed_denied_and_unsupported_keep_declared_weight() {
        use crate::routes::common::{classify_l4_selected_backend, L4SelectedBackend};
        let unit = TcpRouteUnit::from_route(mixed_weight_route());
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
