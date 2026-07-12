//! Resource Handlers
//!
//! This module contains ProcessorHandler implementations for each resource type.
//! Handlers are stateless and only define processing logic - state management
//! is handled by ResourceProcessor.

pub mod auth_spec;
mod backend_tls_policy;
pub(crate) mod config_data_ref_mark;
mod configmap;
pub(crate) mod dns_resolver_guard;
mod edgion_acme;
mod edgion_backend_traffic_policy;
mod edgion_config_data;
mod edgion_gateway_config;
mod edgion_plugins;
mod edgion_stream_plugins;
mod edgion_tls;
mod endpoint_slice;
mod gateway;
mod gateway_class;
mod grpc_route;
pub(crate) mod hostname_resolution;
mod http_route;
mod link_sys;
mod plugins;
mod reference_grant;
pub(crate) mod route_delegation;
pub(crate) mod route_utils;
pub(crate) mod san_guard;
mod secret;
mod service;
mod tcp_route;
mod tls_route;
mod udp_route;

pub use backend_tls_policy::{BackendTlsPolicyAlpha3Handler, BackendTlsPolicyHandler};
pub use configmap::ConfigMapHandler;
pub use edgion_acme::EdgionAcmeHandler;
pub use edgion_backend_traffic_policy::EdgionBackendTrafficPolicyHandler;
pub use edgion_config_data::EdgionConfigDataHandler;
pub use edgion_gateway_config::EdgionGatewayConfigHandler;
pub use edgion_plugins::EdgionPluginsHandler;
pub use edgion_stream_plugins::EdgionStreamPluginsHandler;
pub use edgion_tls::EdgionTlsHandler;
pub use endpoint_slice::EndpointSliceHandler;
pub use gateway::GatewayHandler;
pub use gateway_class::GatewayClassHandler;
pub use grpc_route::GrpcRouteHandler;
pub use http_route::HttpRouteHandler;
pub use link_sys::LinkSysHandler;
pub use reference_grant::{ReferenceGrantBetaHandler, ReferenceGrantHandler};
pub use secret::SecretHandler;
pub use service::ServiceHandler;
pub use tcp_route::TcpRouteHandler;
pub use tls_route::{TlsRouteAlpha3Handler, TlsRouteHandler};
pub use udp_route::UdpRouteHandler;

use crate::conf_mgr::sync_runtime::resource_processor::HandlerContext;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::ResourceKind;

/// Requeue parent Gateways referenced by route parentRefs.
pub(crate) async fn requeue_parent_gateways(
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
        ctx.requeue("Gateway", gateway_key).await;
    }
}

/// Remove a route from the attached-route tracker.
/// Returns true if the route was tracked (had entries to remove).
pub(crate) fn remove_from_attached_route_tracker(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    runtime: &ControllerRuntime,
) -> bool {
    let route_key = format!("{}/{}", route_ns, route_name);
    runtime.attached_route_tracker.remove_route(route_kind, &route_key)
}

/// Update the Gateway → Route index so that Gateway changes can requeue affected routes.
pub(crate) fn update_gateway_route_index(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    parent_refs: Option<&Vec<ParentReference>>,
    runtime: &ControllerRuntime,
) {
    let route_key = format!("{}/{}", route_ns, route_name);
    let empty_refs = Vec::new();
    let refs = parent_refs.unwrap_or(&empty_refs);
    runtime
        .gateway_route_index
        .update_route(route_kind, &route_key, refs, route_ns);
}

/// Remove a route from the Gateway → Route index.
pub(crate) fn remove_from_gateway_route_index(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    runtime: &ControllerRuntime,
) {
    let route_key = format!("{}/{}", route_ns, route_name);
    runtime.gateway_route_index.remove_route(route_kind, &route_key);
}

/// Requeue BackendTLSPolicies bound to backend Services of this route.
///
/// When a route changes, any BTP targeting its backend Services needs to
/// re-resolve its ancestor Gateways (the route may have changed parentRefs).
///
/// Chain: Route → ServiceRefManager(reverse: route→services) → BackendTLSPolicyTargetManager(forward: service→BTPs)
pub(crate) async fn requeue_backend_tls_policies_for_route(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    ctx: &HandlerContext,
) {
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceRef;

    let route_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());
    let service_keys = ctx.runtime().service_ref_mgr.get_dependencies(&route_ref.key());

    if service_keys.is_empty() {
        return;
    }

    let target_manager = ctx.runtime().backend_tls_policy.target_mgr();
    for service_key in &service_keys {
        let btps = target_manager.get_refs(service_key);
        for btp_ref in btps {
            tracing::debug!(
                route = %route_ref.resource_key(),
                btp = %btp_ref.resource_key(),
                service = %service_key,
                "Requeuing BackendTLSPolicy due to route change"
            );
            ctx.requeue(btp_ref.kind_str(), btp_ref.resource_key()).await;
        }
    }
}
