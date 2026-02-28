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
mod http_route;
mod link_sys;
mod plugin_metadata;
mod reference_grant;
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

use crate::core::conf_mgr::sync_runtime::resource_processor::HandlerContext;
use crate::types::resources::common::ParentReference;

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
