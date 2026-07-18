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
pub(crate) mod grpc_route_delegation;
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

use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef, SecretRefManager};
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
///
/// Returns the gateway keys the route no longer references (dropped by this
/// update) — the caller MUST requeue them via `ctx.requeue("Gateway", key)`,
/// since a coalesced delete->recreate in the workqueue skips `handler.on_delete`
/// for the previous generation and would otherwise leave the dropped Gateway's
/// derived state (e.g. `attachedRoutes`) stale.
pub(crate) fn update_gateway_route_index(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    parent_refs: Option<&Vec<ParentReference>>,
    runtime: &ControllerRuntime,
) -> Vec<String> {
    let route_key = format!("{}/{}", route_ns, route_name);
    let empty_refs = Vec::new();
    let refs = parent_refs.unwrap_or(&empty_refs);
    runtime
        .gateway_route_index
        .update_route(route_kind, &route_key, refs, route_ns)
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

/// Collect BackendTLSPolicy refs registered against a backend Service, matching
/// both the bare `"ns/svc"` target key and any section-scoped
/// `"ns/svc::sectionName"` key.
///
/// BTP target registrations may carry a `::sectionName` suffix
/// (`make_target_conflict_key`), which a plain `get_refs("ns/svc")` exact lookup
/// would miss. Reduce every registered target key to its base form before
/// matching — the same reduction the EBTP requeue block in
/// `backend_tls_policy.rs` uses.
fn btp_refs_for_service(target_manager: &SecretRefManager, service_key: &str) -> Vec<ResourceRef> {
    let mut refs = Vec::new();
    for target_key in target_manager.all_source_keys() {
        let base = target_key.split("::").next().unwrap_or(&target_key);
        if base == service_key {
            refs.extend(target_manager.get_refs(&target_key));
        }
    }
    refs
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
    let route_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());
    let service_keys = ctx.runtime().service_ref_mgr.get_dependencies(&route_ref.key());

    if service_keys.is_empty() {
        return;
    }

    let target_manager = ctx.runtime().backend_tls_policy.target_mgr();
    for service_key in &service_keys {
        let btps = btp_refs_for_service(target_manager, service_key);
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

/// Requeue BackendTLSPolicies bound to Services this route no longer references.
///
/// Companion to `requeue_backend_tls_policies_for_route`: that helper reads the
/// route's *current* backend Services (the apply path re-registers them in
/// `parse` before `on_change` runs), so a Service dropped from backendRefs would
/// never have its BTPs requeued to re-resolve ancestor Gateways. Feed it the
/// dropped Service keys returned by `register_service_backend_refs`.
pub(crate) async fn requeue_backend_tls_policies_for_services(dropped_service_keys: &[String], ctx: &HandlerContext) {
    if dropped_service_keys.is_empty() {
        return;
    }
    let target_manager = ctx.runtime().backend_tls_policy.target_mgr();
    for service_key in dropped_service_keys {
        for btp_ref in btp_refs_for_service(target_manager, service_key) {
            tracing::debug!(
                btp = %btp_ref.resource_key(),
                service = %service_key,
                "Requeuing BackendTLSPolicy: route dropped this backend Service"
            );
            ctx.requeue(btp_ref.kind_str(), btp_ref.resource_key()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::{make_target_conflict_key, BidirectionalRefManager};

    fn btp_ref(ns: &str, name: &str) -> ResourceRef {
        ResourceRef::new(ResourceKind::BackendTLSPolicy, Some(ns.to_string()), name.to_string())
    }

    /// `btp_refs_for_service` must match both the bare `"ns/svc"` registration and
    /// any section-scoped `"ns/svc::section"` registration for the same Service.
    #[test]
    fn btp_refs_for_service_matches_bare_and_section_keys() {
        let mgr: SecretRefManager = BidirectionalRefManager::with_component("test");

        let bare = btp_ref("ns", "btp-bare");
        let sec80 = btp_ref("ns", "btp-80");
        let sec443 = btp_ref("ns", "btp-443");

        mgr.add_ref(make_target_conflict_key("ns", "svc", None), bare.clone());
        mgr.add_ref(make_target_conflict_key("ns", "svc", Some("port-80")), sec80.clone());
        mgr.add_ref(make_target_conflict_key("ns", "svc", Some("port-443")), sec443.clone());
        // A different Service that shares the "svc" name prefix must NOT match —
        // neither its bare key nor a section-scoped key may over-match "ns/svc".
        mgr.add_ref(
            make_target_conflict_key("ns", "svc-other", None),
            btp_ref("ns", "btp-other"),
        );
        mgr.add_ref(
            make_target_conflict_key("ns", "svc-other", Some("s")),
            btp_ref("ns", "btp-other-sec"),
        );

        let mut got: Vec<String> = btp_refs_for_service(&mgr, "ns/svc")
            .iter()
            .map(|r| r.resource_key())
            .collect();
        got.sort();

        assert_eq!(got, vec!["ns/btp-443", "ns/btp-80", "ns/btp-bare"]);
    }

    /// A Service with no BTP targets yields no refs.
    #[test]
    fn btp_refs_for_service_no_match_is_empty() {
        let mgr: SecretRefManager = BidirectionalRefManager::with_component("test");
        mgr.add_ref(
            make_target_conflict_key("ns", "svc", Some("port-80")),
            btp_ref("ns", "btp-80"),
        );

        assert!(btp_refs_for_service(&mgr, "ns/nomatch").is_empty());
    }
}
