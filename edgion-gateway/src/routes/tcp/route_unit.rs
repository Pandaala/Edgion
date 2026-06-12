use std::sync::Arc;

use crate::lb::BackendSelector;
use crate::routes::common::{is_core_service_backend_ref, resolve_stream_plugin_store_key, resolve_tcp_keepalive};
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
    /// rule's backend refs. None of this state lives on the schema anymore.
    pub fn from_route(route: Arc<TCPRoute>) -> Self {
        let route_key = route.key_name();
        let stream_plugin_store_key = resolve_stream_plugin_store_key(&route.metadata);
        let tcp_keepalive = resolve_tcp_keepalive(route.metadata.annotations.as_ref(), &route_key);
        let backend_finder = build_backend_selector(&route);
        TcpRouteUnit {
            route,
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

/// Build a WRR selector over the first rule's core-service backend refs.
///
/// Mirrors the previous `conf_handler_impl::initialize_route` logic so the
/// selector contents are unchanged by the decoupling.
fn build_backend_selector(route: &TCPRoute) -> Arc<BackendSelector<TCPBackendRef>> {
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
}
