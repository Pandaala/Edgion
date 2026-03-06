//! Resource Handlers
//!
//! This module contains ProcessorHandler implementations for each resource type.
//! Handlers are stateless and only define processing logic - state management
//! is handled by ResourceProcessor.

mod backend_tls_policy;
mod edgion_acme;
mod edgion_gateway_config;
mod edgion_plugins;
mod edgion_stream_plugins;
mod edgion_tls;
mod endpoint_slice;
mod endpoints;
mod gateway;
mod gateway_class;
mod grpc_route;
pub(crate) mod hostname_resolution;
mod http_route;
mod link_sys;
mod plugin_metadata;
mod reference_grant;
pub(crate) mod route_utils;
mod secret;
mod service;
mod tcp_route;
mod tls_route;
mod udp_route;

pub use backend_tls_policy::BackendTlsPolicyHandler;
pub use edgion_acme::EdgionAcmeHandler;
pub use edgion_gateway_config::EdgionGatewayConfigHandler;
pub use edgion_plugins::EdgionPluginsHandler;
pub use edgion_stream_plugins::EdgionStreamPluginsHandler;
pub use edgion_tls::EdgionTlsHandler;
pub use endpoint_slice::EndpointSliceHandler;
pub use endpoints::EndpointsHandler;
pub use gateway::GatewayHandler;
pub use gateway_class::GatewayClassHandler;
pub use grpc_route::GrpcRouteHandler;
pub use http_route::HttpRouteHandler;
pub use link_sys::LinkSysHandler;
pub use plugin_metadata::PluginMetadataHandler;
pub use reference_grant::ReferenceGrantHandler;
pub use secret::SecretHandler;
pub use service::ServiceHandler;
pub use tcp_route::TcpRouteHandler;
pub use tls_route::TlsRouteHandler;
pub use udp_route::UdpRouteHandler;

use std::collections::HashSet;

use crate::core::conf_mgr::sync_runtime::resource_processor::attached_route_tracker::Attachment;
use crate::core::conf_mgr::sync_runtime::resource_processor::get_attached_route_tracker;
use crate::core::conf_mgr::sync_runtime::resource_processor::get_gateway_route_index;
use crate::core::conf_mgr::sync_runtime::resource_processor::HandlerContext;
use crate::types::resources::common::ParentReference;
use crate::types::ResourceKind;

/// Requeue parent Gateways referenced by route parentRefs.
pub(crate) fn requeue_parent_gateways(
    parent_refs: Option<&Vec<ParentReference>>,
    route_ns: &str,
    ctx: &HandlerContext,
) {
    let Some(parent_refs) = parent_refs else {
        return;
    };

    for parent_ref in parent_refs {
        let parent_group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");
        let parent_kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
        if parent_group != "gateway.networking.k8s.io" || parent_kind != "Gateway" {
            continue;
        }

        let gateway_key = parent_ref.build_parent_key(Some(route_ns));
        ctx.requeue("Gateway", gateway_key);
    }
}

/// Record a route's parentRef attachments in the global tracker.
///
/// Only reads parentRef fields (gateway ns/name, optional sectionName).
/// Does NOT look up Gateway — works even if the target Gateway hasn't arrived yet.
///
/// Returns true if the attachments actually changed.
pub(crate) fn update_attached_route_tracker(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    parent_refs: Option<&Vec<ParentReference>>,
) -> bool {
    let route_key = format!("{}/{}", route_ns, route_name);
    let Some(parent_refs) = parent_refs else {
        return get_attached_route_tracker().remove_route(route_kind, &route_key);
    };

    let mut attachments = HashSet::new();

    for parent_ref in parent_refs {
        let parent_group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");
        let parent_kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
        if parent_group != "gateway.networking.k8s.io" || parent_kind != "Gateway" {
            continue;
        }

        let gateway_ns = parent_ref.namespace.as_deref().unwrap_or(route_ns);
        let gateway_name = &parent_ref.name;

        attachments.insert(Attachment {
            gateway_key: format!("{}/{}", gateway_ns, gateway_name),
            listener_name: parent_ref.section_name.as_deref().unwrap_or("").to_string(),
        });
    }

    get_attached_route_tracker().update_route(route_kind, &route_key, attachments)
}

/// Remove a route from the attached-route tracker.
/// Returns true if the route was tracked (had entries to remove).
pub(crate) fn remove_from_attached_route_tracker(route_kind: ResourceKind, route_ns: &str, route_name: &str) -> bool {
    let route_key = format!("{}/{}", route_ns, route_name);
    get_attached_route_tracker().remove_route(route_kind, &route_key)
}

/// Update the Gateway → Route index so that Gateway changes can requeue affected routes.
pub(crate) fn update_gateway_route_index(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    parent_refs: Option<&Vec<ParentReference>>,
) {
    let route_key = format!("{}/{}", route_ns, route_name);
    let empty_refs = Vec::new();
    let refs = parent_refs.unwrap_or(&empty_refs);
    get_gateway_route_index().update_route(route_kind, &route_key, refs, route_ns);
}

/// Remove a route from the Gateway → Route index.
pub(crate) fn remove_from_gateway_route_index(route_kind: ResourceKind, route_ns: &str, route_name: &str) {
    let route_key = format!("{}/{}", route_ns, route_name);
    get_gateway_route_index().remove_route(route_kind, &route_key);
}
