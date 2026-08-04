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
mod edgion_ai_service;
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
pub use edgion_ai_service::EdgionAIServiceHandler;
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
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::ResourceKind;
use std::collections::{BTreeMap, BTreeSet};

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

/// EdgionBackendTrafficPolicy refs registered under a Service key.
///
/// EBTP `parse` registers every policy (valid or not) in `service_ref_mgr`
/// under its target Service keys, so this covers invalid policies too —
/// their per-ancestor `Accepted=False` status must also stay fresh.
/// Filtering by kind is load-bearing: routes register under the same keys,
/// and requeuing a route from its own change path would loop forever.
fn ebtp_refs_for_service(service_ref_mgr: &SecretRefManager, service_key: &str) -> Vec<ResourceRef> {
    ebtp_refs_for_target(service_ref_mgr, service_key)
}

/// EdgionBackendTrafficPolicy refs registered under any canonical target key.
fn ebtp_refs_for_target(target_ref_mgr: &SecretRefManager, target_key: &str) -> Vec<ResourceRef> {
    target_ref_mgr
        .get_refs(target_key)
        .into_iter()
        .filter(|r| r.kind == ResourceKind::EdgionBackendTrafficPolicy)
        .collect()
}

/// Resolve the ancestor Gateways for typed EBTP targets.
///
/// Service targets walk the existing Service reference manager. AI targets
/// walk the distinct AI-service manager, which prevents same-named Service and
/// EdgionAIService resources from sharing dependency or ancestor edges.
pub(crate) fn resolve_policy_ancestor_gateways_for_targets(
    targets: &[PolicyTargetKey],
    policy_key: &str,
    ctx: &HandlerContext,
) -> BTreeMap<String, BTreeSet<PolicyTargetKey>> {
    const MAX_POLICY_ANCESTORS: usize = 16;

    let route_index = &ctx.runtime().gateway_route_index;
    let mut map: BTreeMap<String, BTreeSet<PolicyTargetKey>> = BTreeMap::new();
    for target in targets {
        let refs = match target.target_kind {
            PolicyTargetKind::Service => ctx
                .runtime()
                .service_ref_mgr
                .get_refs(&format!("{}/{}", target.namespace, target.name)),
            PolicyTargetKind::EdgionAIService => ctx.runtime().ai_service_ref_mgr.get_refs(&target.to_string()),
        };
        for route_ref in refs {
            for gateway_key in route_index.get_gateways_for_route(route_ref.kind, &route_ref.resource_key()) {
                map.entry(gateway_key).or_default().insert(target.clone());
            }
        }
    }

    if map.len() > MAX_POLICY_ANCESTORS {
        tracing::warn!(
            policy = %policy_key,
            total = map.len(),
            "policy ancestors truncated to {} per Gateway API limit",
            MAX_POLICY_ANCESTORS,
        );
        let overflow: Vec<String> = map.keys().skip(MAX_POLICY_ANCESTORS).cloned().collect();
        for key in overflow {
            map.remove(&key);
        }
    }
    map
}

/// Resolve the ancestor Gateways for a Service-targeting policy, per
/// GEP-2648: the ancestor of a direct policy is the Gateway the traffic
/// enters through.
///
/// Chain (all in-memory, workqueue Rule 5 safe):
///   targetRef(Service) -> ServiceRefManager (service -> routes)
///                      -> GatewayRouteIndex (route -> gateways)
///
/// Returns gateway key -> the target Service keys ("ns/svc") whose routes
/// attach to that gateway, so callers can compute per-ancestor conditions
/// (e.g. Conflicted only on the ancestors the conflicted Service reaches).
/// Sorted (BTreeMap) and capped at 16 ancestors per the Gateway API
/// `maxItems=16` on `status.ancestors`; truncation keeps the smallest keys
/// so the same 16 are selected on every reconcile.
///
/// Non-route refs under a Service key (policies register there too for
/// Service-change requeue) resolve to zero gateways in the route index and
/// therefore drop out naturally.
pub(crate) fn resolve_policy_ancestor_gateways(
    policy_ns: &str,
    target_service_names: &[&str],
    policy_key: &str,
    ctx: &HandlerContext,
) -> BTreeMap<String, BTreeSet<String>> {
    let targets: Vec<PolicyTargetKey> = target_service_names
        .iter()
        .map(|name| PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: policy_ns.to_string(),
            name: (*name).to_string(),
        })
        .collect();
    resolve_policy_ancestor_gateways_for_targets(&targets, policy_key, ctx)
        .into_iter()
        .map(|(gateway, targets)| {
            let service_keys = targets
                .into_iter()
                .map(|target| format!("{}/{}", target.namespace, target.name))
                .collect();
            (gateway, service_keys)
        })
        .collect()
}

/// Requeue backend policies (BackendTLSPolicy + EdgionBackendTrafficPolicy)
/// bound to backend Services of this route.
///
/// When a route changes, any backend policy targeting its backend Services
/// needs to re-resolve its ancestor Gateways (the route may have changed
/// parentRefs).
///
/// Chain: Route → ServiceRefManager(reverse: route→services) → {BackendTLSPolicyTargetManager, service_ref_mgr}(forward: service→policies)
pub(crate) async fn requeue_backend_policies_for_route(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    ctx: &HandlerContext,
) {
    let route_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());
    let service_keys = ctx.runtime().service_ref_mgr.get_dependencies(&route_ref.key());

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

        for ebtp_ref in ebtp_refs_for_service(&ctx.runtime().service_ref_mgr, service_key) {
            tracing::debug!(
                route = %route_ref.resource_key(),
                policy = %ebtp_ref.resource_key(),
                service = %service_key,
                "Requeuing EdgionBackendTrafficPolicy due to route change"
            );
            ctx.requeue(ebtp_ref.kind_str(), ebtp_ref.resource_key()).await;
        }
    }

    let ai_target_keys = ctx.runtime().ai_service_ref_mgr.get_dependencies(&route_ref.key());
    requeue_backend_policies_for_ai_targets(&ai_target_keys, ctx).await;
}

/// Requeue EBTPs bound to typed EdgionAIService targets.
///
/// Callers pass the union of old/new route target keys so parentRef,
/// delegation, and backend replacement all refresh affected ancestors.
pub(crate) async fn requeue_backend_policies_for_ai_targets(ai_target_keys: &[String], ctx: &HandlerContext) {
    for target_key in ai_target_keys {
        for policy_ref in ebtp_refs_for_target(&ctx.runtime().ai_service_ref_mgr, target_key) {
            tracing::debug!(
                policy = %policy_ref.resource_key(),
                ai_service = %target_key,
                "Requeuing EdgionBackendTrafficPolicy due to AI route target change"
            );
            ctx.requeue(policy_ref.kind_str(), policy_ref.resource_key()).await;
        }
    }
}

/// Requeue backend policies (BackendTLSPolicy + EdgionBackendTrafficPolicy)
/// bound to Services this route no longer references.
///
/// Companion to `requeue_backend_policies_for_route`: that helper reads the
/// route's *current* backend Services (the apply path re-registers them in
/// `parse` before `on_change` runs), so a Service dropped from backendRefs would
/// never have its policies requeued to re-resolve ancestor Gateways. Feed it the
/// dropped Service keys returned by `register_service_backend_refs`.
pub(crate) async fn requeue_backend_policies_for_services(dropped_service_keys: &[String], ctx: &HandlerContext) {
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

        for ebtp_ref in ebtp_refs_for_service(&ctx.runtime().service_ref_mgr, service_key) {
            tracing::debug!(
                policy = %ebtp_ref.resource_key(),
                service = %service_key,
                "Requeuing EdgionBackendTrafficPolicy: route dropped this backend Service"
            );
            ctx.requeue(ebtp_ref.kind_str(), ebtp_ref.resource_key()).await;
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

    /// The shared ancestor resolver walks Service -> Routes (service_ref_mgr)
    /// -> Gateways (gateway_route_index), maps each gateway to the target
    /// Service keys that reach it, and ignores non-route refs (policies also
    /// register under Service keys).
    #[test]
    fn resolve_policy_ancestor_gateways_maps_gateways_to_services() {
        use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
        use crate::conf_mgr::ControllerRuntime;
        use edgion_resources::resources::common::ParentReference;
        use edgion_resources::ResourceKind;
        use std::sync::Arc;

        let rt = Arc::new(ControllerRuntime::default());
        let ctx = crate::conf_mgr::sync_runtime::resource_processor::HandlerContext::new(
            None,
            None,
            TriggerChain::default(),
            5,
            20,
            rt.clone(),
        );

        let gw_parent = |name: &str| ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("Gateway".to_string()),
            namespace: Some("ns".to_string()),
            name: name.to_string(),
            section_name: None,
            port: None,
        };

        // svc-a reaches gw-a via route-a; svc-b reaches gw-b via route-b.
        for (svc, route, gw) in [("svc-a", "route-a", "gw-a"), ("svc-b", "route-b", "gw-b")] {
            rt.service_ref_mgr.add_ref(
                format!("ns/{svc}"),
                ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), route.to_string()),
            );
            rt.gateway_route_index.update_route(
                ResourceKind::HTTPRoute,
                &format!("ns/{route}"),
                &[gw_parent(gw)],
                "ns",
            );
        }
        // A policy ref registered under the same Service key must be ignored.
        rt.service_ref_mgr.add_ref(
            "ns/svc-a".to_string(),
            ResourceRef::new(
                ResourceKind::EdgionBackendTrafficPolicy,
                Some("ns".to_string()),
                "some-policy".to_string(),
            ),
        );

        let map = resolve_policy_ancestor_gateways("ns", &["svc-a", "svc-b"], "ns/p", &ctx);
        assert_eq!(map.len(), 2);
        assert!(map["ns/gw-a"].contains("ns/svc-a"));
        assert!(!map["ns/gw-a"].contains("ns/svc-b"));
        assert!(map["ns/gw-b"].contains("ns/svc-b"));

        // A target Service nobody routes to contributes no ancestor.
        let map = resolve_policy_ancestor_gateways("ns", &["svc-unrouted"], "ns/p", &ctx);
        assert!(map.is_empty());
    }

    #[test]
    fn typed_ancestor_resolver_separates_same_named_service_and_ai_service() {
        use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
        use crate::conf_mgr::ControllerRuntime;
        use edgion_resources::resources::common::ParentReference;
        use std::sync::Arc;

        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = crate::conf_mgr::sync_runtime::resource_processor::HandlerContext::new(
            None,
            None,
            TriggerChain::default(),
            5,
            20,
            runtime.clone(),
        );
        let service_route = ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "service".to_string());
        let ai_route = ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "ai".to_string());
        let service_target = PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: "ns".to_string(),
            name: "shared".to_string(),
        };
        let ai_target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionAIService,
            namespace: "ns".to_string(),
            name: "shared".to_string(),
        };
        runtime
            .service_ref_mgr
            .add_ref("ns/shared".to_string(), service_route.clone());
        runtime
            .ai_service_ref_mgr
            .add_ref(ai_target.to_string(), ai_route.clone());

        let parent = |name: &str| ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("Gateway".to_string()),
            namespace: Some("ns".to_string()),
            name: name.to_string(),
            section_name: None,
            port: None,
        };
        runtime.gateway_route_index.update_route(
            ResourceKind::HTTPRoute,
            &service_route.resource_key(),
            &[parent("service-gw")],
            "ns",
        );
        runtime.gateway_route_index.update_route(
            ResourceKind::HTTPRoute,
            &ai_route.resource_key(),
            &[parent("ai-gw")],
            "ns",
        );

        let map = resolve_policy_ancestor_gateways_for_targets(
            &[service_target.clone(), ai_target.clone()],
            "ns/policy",
            &ctx,
        );
        assert_eq!(map["ns/service-gw"], BTreeSet::from([service_target]));
        assert_eq!(map["ns/ai-gw"], BTreeSet::from([ai_target]));
    }

    /// `ebtp_refs_for_service` returns only EdgionBackendTrafficPolicy refs
    /// registered under a Service key. Routes register under the same keys and
    /// MUST be excluded — requeuing them from a route-change path would make a
    /// route requeue itself (an infinite route->route loop).
    #[test]
    fn ebtp_refs_for_service_filters_out_route_refs() {
        let mgr = BidirectionalRefManager::with_component("test_service_ref");
        let policy = ResourceRef::new(
            ResourceKind::EdgionBackendTrafficPolicy,
            Some("ns".to_string()),
            "p1".to_string(),
        );
        let route = ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "r1".to_string());
        mgr.add_ref("ns/svc".to_string(), policy.clone());
        mgr.add_ref("ns/svc".to_string(), route);

        let refs = ebtp_refs_for_service(&mgr, "ns/svc");
        assert_eq!(refs, vec![policy]);
        assert!(ebtp_refs_for_service(&mgr, "ns/other").is_empty());
    }
}
