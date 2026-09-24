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
            route
                .spec
                .current_status
                .as_ref()
                .map(|status| status.parents.as_slice()),
            Some(namespace),
            route.spec.resolved_status_controller.as_deref(),
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
#[path = "../../../tests/unit/routes/udp/route_unit_test.rs"]
mod tests;
