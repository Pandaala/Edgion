use std::sync::Arc;

use crate::lb::BackendSelector;
use crate::routes::common::{is_core_service_backend_ref, resolve_stream_plugin_store_key};
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
    /// lives on the schema anymore.
    pub fn from_route(route: Arc<UDPRoute>) -> Self {
        let stream_plugin_store_key = resolve_stream_plugin_store_key(&route.metadata);
        let backend_finder = build_backend_selector(&route);
        UdpRouteUnit {
            route,
            backend_finder,
            stream_plugin_store_key,
        }
    }

    /// Delegate to the inner route's resource key (e.g. `"default/my-route"`).
    pub fn key_name(&self) -> String {
        self.route.key_name()
    }
}

/// Build a WRR selector over the first rule's core-service backend refs.
fn build_backend_selector(route: &UDPRoute) -> Arc<BackendSelector<UDPBackendRef>> {
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
                        extension_info: Default::default(),
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
}
