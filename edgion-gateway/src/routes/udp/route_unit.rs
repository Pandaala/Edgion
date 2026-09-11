use crate::routes::common::{filter_accepted_parent_refs, resolve_parent_refs};
use crate::runtime::matching::route::ResolvedParentRef;
use std::sync::Arc;

use crate::lb::BackendSelector;
use crate::routes::common::resolve_stream_plugin_store_key;
use edgion_resources::resources::udp_route::UDPBackendRef;
use edgion_resources::resources::UDPRoute;
use edgion_resources::ResourceMeta;

/// Gateway-side compiled companion for a UDPRoute.
///
/// Holds the route schema plus the compiled runtime that cannot live in
/// `src/types` (BackendSelector, stream-plugin store key). Built once per route
/// from the cached `Arc<UDPRoute>`; the inner `route` field keeps the full
/// schema accessible for the data plane.
///
/// Per-protocol note: UDP is port-based with no hostname fan-out — one unit per
/// route suffices. `backend_finder` is built for the first rule only, matching
/// the data-plane read path (`rules.first()`).
pub struct UdpRouteUnit {
    /// Accepted parent identities resolved once for logical Gateway admission.
    pub resolved_parent_refs: Vec<ResolvedParentRef>,
    pub route: Arc<UDPRoute>,
    /// WRR selector over the first rule's backends.
    pub backend_finder: Arc<BackendSelector<UDPBackendRef>>,
    /// Store key for dynamic stream-plugin lookup (from route annotations).
    pub stream_plugin_store_key: Option<String>,
}

impl UdpRouteUnit {
    /// Build a unit from a cached route.
    ///
    /// Resolves the stream-plugin store key from the route's metadata (single
    /// source of truth shared with the HTTP/TCP/TLS conf-handlers) and builds
    /// the WRR selector from the first rule's backend refs. None of this state
    /// lives on the schema.
    pub fn from_route(route: Arc<UDPRoute>) -> Self {
        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let accepted = filter_accepted_parent_refs(
            route.spec.parent_refs.as_deref(),
            route.status.as_ref().map(|status| status.parents.as_slice()),
            Some(namespace),
        )
        .unwrap_or_default();
        let resolved_parent_refs = resolve_parent_refs(&accepted, namespace);

        let stream_plugin_store_key = resolve_stream_plugin_store_key(&route.metadata);
        let backend_finder = build_backend_selector(&route);
        UdpRouteUnit {
            route,
            resolved_parent_refs,
            backend_finder,
            stream_plugin_store_key,
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
fn build_backend_selector(route: &UDPRoute) -> Arc<BackendSelector<UDPBackendRef>> {
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
    use edgion_resources::resources::udp_route::{UDPBackendRef, UDPRouteRule, UDPRouteSpec};

    fn make_route(annotations: Option<std::collections::BTreeMap<String, String>>) -> Arc<UDPRoute> {
        Arc::new(UDPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("r".to_string()),
                annotations,
                ..Default::default()
            },
            spec: UDPRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some("default".to_string()),
                    name: "gw".to_string(),
                    section_name: Some("udp".to_string()),
                    port: None,
                }]),
                rules: Some(vec![UDPRouteRule {
                    backend_refs: Some(vec![UDPBackendRef {
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
    fn test_stream_plugin_store_key_resolved_on_unit() {
        let mut a = std::collections::BTreeMap::new();
        a.insert("edgion.io/edgion-stream-plugins".to_string(), "ns/plugins".to_string());
        let unit = UdpRouteUnit::from_route(make_route(Some(a)));
        assert_eq!(unit.stream_plugin_store_key, Some("ns/plugins".to_string()));
    }

    #[test]
    fn test_no_annotation_leaves_store_key_none() {
        let unit = UdpRouteUnit::from_route(make_route(None));
        assert!(unit.stream_plugin_store_key.is_none());
    }

    #[test]
    fn test_backend_finder_built_from_first_rule() {
        let unit = UdpRouteUnit::from_route(make_route(None));
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
        let unit = UdpRouteUnit::from_route(route);
        let selected = unit
            .backend_finder
            .select()
            .expect("denied core Service stays selectable");
        assert!(selected.ref_denied.is_some());
    }

    fn udp_backend(name: &str, group: Option<&str>, kind: Option<&str>, denied: bool) -> UDPBackendRef {
        UDPBackendRef {
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

    fn mixed_weight_route() -> Arc<UDPRoute> {
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
                udp_backend("allowed", None, None, false),
                udp_backend("denied", None, None, true),
                udp_backend("invalid", Some("apps"), Some("Deployment"), false),
            ];
        }
        route
    }

    #[test]
    fn mixed_denied_and_unsupported_keep_declared_weight() {
        use crate::routes::common::{classify_l4_selected_backend, L4SelectedBackend};
        let unit = UdpRouteUnit::from_route(mixed_weight_route());
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
