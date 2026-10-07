use crate::routes::common::{filter_accepted_parent_refs, resolve_parent_refs};
use crate::runtime::matching::route::ResolvedParentRef;
use std::sync::Arc;

use crate::backends::{CompiledL4Backends, SelectedL4Backend};
use crate::routes::common::resolve_stream_plugin_store_key;
use edgion_resources::resources::refs::ReferenceError;
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
/// The schema and compiled selection cannot be replaced independently.
/// ```compile_fail
/// use edgion_gateway::routes::udp::UdpRouteUnit;
/// fn replace(unit: &mut UdpRouteUnit) { unit.route = unit.route().clone(); }
/// ```
pub struct UdpRouteUnit {
    /// Accepted parent identities resolved once for logical Gateway admission.
    resolved_parent_refs: Vec<ResolvedParentRef>,
    route: Arc<UDPRoute>,
    /// WRR selector over the first rule's backends.
    backend_finder: Arc<CompiledL4Backends>,
    /// Store key for dynamic stream-plugin lookup (from route annotations).
    stream_plugin_store_key: Option<String>,
}

impl UdpRouteUnit {
    /// Build a unit from a cached route.
    ///
    /// Resolves the stream-plugin store key from the route's metadata (single
    /// source of truth shared with the HTTP/TCP/TLS conf-handlers) and builds
    /// the WRR selector from the first rule's backend refs. None of this state
    /// lives on the schema.
    pub fn from_route(route: Arc<UDPRoute>) -> Result<Self, ReferenceError> {
        let backend_finder = Arc::new(CompiledL4Backends::from_udp(&route)?);
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
        Ok(UdpRouteUnit {
            route,
            resolved_parent_refs,
            backend_finder,
            stream_plugin_store_key,
        })
    }

    pub fn route(&self) -> &Arc<UDPRoute> {
        &self.route
    }

    pub(crate) fn select_backend(&self) -> Result<SelectedL4Backend<'_>, u32> {
        self.backend_finder.select()
    }

    pub(crate) fn stream_plugin_store_key(&self) -> Option<&str> {
        self.stream_plugin_store_key.as_deref()
    }

    pub(crate) fn resolved_parent_refs(&self) -> &[ResolvedParentRef] {
        &self.resolved_parent_refs
    }

    /// Delegate to the inner route's resource key (e.g. `"default/my-route"`).
    pub fn key_name(&self) -> String {
        self.route.key_name()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/udp/route_unit_test.rs"]
mod tests;
