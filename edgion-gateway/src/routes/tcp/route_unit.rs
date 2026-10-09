use crate::routes::common::{filter_accepted_parent_refs, resolve_parent_refs};
use crate::runtime::matching::route::ResolvedParentRef;
use edgion_resources::resources::refs::plugin_attachment_fields::StreamPluginAttachment;
use std::sync::Arc;

use crate::backends::{CompiledL4Backends, SelectedL4Backend};
use crate::routes::common::{resolve_stream_plugin_store_key, resolve_tcp_keepalive};
use edgion_resources::resources::common::TcpKeepaliveConfig;
use edgion_resources::resources::refs::ReferenceError;
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
/// The schema and compiled selection cannot be replaced independently.
/// ```compile_fail
/// use edgion_gateway::routes::tcp::TcpRouteUnit;
/// fn replace(unit: &mut TcpRouteUnit) { unit.route = unit.route().clone(); }
/// ```
pub struct TcpRouteUnit {
    /// Accepted parent identities resolved once for logical Gateway admission.
    resolved_parent_refs: Vec<ResolvedParentRef>,
    route: Arc<TCPRoute>,
    /// WRR selector over the first rule's backends.
    backend_finder: Arc<CompiledL4Backends>,
    /// Store key for dynamic stream-plugin lookup (from route annotations).
    stream_plugin_store_key: Option<String>,
    /// Resolved TCP keepalive config (from route annotations).
    tcp_keepalive: Option<TcpKeepaliveConfig>,
}

impl TcpRouteUnit {
    /// Build a unit from a cached route.
    ///
    /// Resolves the stream-plugin store key and keepalive config from the
    /// route's metadata/annotations (single source of truth shared with the
    /// HTTP/TLS conf-handlers) and builds the WRR selector from the first
    /// rule's backend refs. None of this state lives on the schema.
    pub fn from_route(route: Arc<TCPRoute>) -> Result<Self, ReferenceError> {
        let backend_finder = Arc::new(CompiledL4Backends::from_tcp(&route)?);
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

        let route_key = route.key_name();
        let stream_plugin_store_key = resolve_stream_plugin_store_key(StreamPluginAttachment::Tcp(&route));
        let tcp_keepalive = resolve_tcp_keepalive(route.metadata.annotations.as_ref(), &route_key);
        Ok(TcpRouteUnit {
            route,
            resolved_parent_refs,
            backend_finder,
            stream_plugin_store_key,
            tcp_keepalive,
        })
    }

    pub fn route(&self) -> &Arc<TCPRoute> {
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

    pub(crate) fn tcp_keepalive(&self) -> Option<&TcpKeepaliveConfig> {
        self.tcp_keepalive.as_ref()
    }

    /// Delegate to the inner route's resource key (e.g. `"default/my-route"`).
    pub fn key_name(&self) -> String {
        self.route.key_name()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/tcp/route_unit_test.rs"]
mod tests;
