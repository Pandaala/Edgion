//! Resource Handlers
//!
//! This module contains ProcessorHandler implementations for each resource type.
//! Handlers are stateless and only define processing logic - state management
//! is handled by ResourceProcessor.

pub mod auth_spec;
mod backend_capability;
mod backend_tls_policy;
pub(crate) mod config_data_ref_mark;
mod configmap;
pub(crate) mod dns_resolver_guard;
mod edgion_acme;
mod edgion_backend;
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
pub use edgion_backend::EdgionBackendHandler;
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

/// BackendTLSPolicy refs registered under a Service key.
///
/// `parse_policy` registers every BackendTLSPolicy in `service_ref_mgr` under
/// its canonical target Service key *before* checking whether the target or the
/// CA refs resolve, so this also covers a policy whose refs are broken — it
/// still publishes ancestor status, and that status must stay fresh when the
/// target's routes or Gateways change. Do NOT use the conflict index
/// (`backend_tls_policy.target_mgr()`) here: it deliberately admits only
/// policies eligible for arbitration, so a policy with an unresolvable CA would
/// never be requeued and its ancestors would freeze at whatever the route index
/// held the first time it was processed.
///
/// Filtering by kind is load-bearing: routes and EBTPs register under the same
/// keys, and requeuing a route from its own change path would loop forever.
fn btp_refs_for_service(service_ref_mgr: &SecretRefManager, service_key: &str) -> Vec<ResourceRef> {
    service_ref_mgr
        .get_refs(service_key)
        .into_iter()
        .filter(|r| r.kind == ResourceKind::BackendTLSPolicy)
        .collect()
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
/// walk the distinct typed-backend manager, which prevents same-named Service and
/// EdgionBackend resources from sharing dependency or ancestor edges.
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
            PolicyTargetKind::EdgionBackend => ctx.runtime().backend_ref_mgr.get_refs(&target.to_string()),
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

#[cfg(test)]
fn resolve_policy_ancestor_gateways(
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

    for service_key in &service_keys {
        for btp_ref in btp_refs_for_service(&ctx.runtime().service_ref_mgr, service_key) {
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

    let backend_target_keys = ctx.runtime().backend_ref_mgr.get_dependencies(&route_ref.key());
    requeue_backend_policies_for_backend_targets(&backend_target_keys, ctx).await;
}

/// Requeue EBTPs bound to typed EdgionBackend targets.
///
/// Callers pass the union of old/new route target keys so parentRef,
/// delegation, and backend replacement all refresh affected ancestors.
pub(crate) async fn requeue_backend_policies_for_backend_targets(backend_target_keys: &[String], ctx: &HandlerContext) {
    for target_key in backend_target_keys {
        for policy_ref in ctx
            .runtime()
            .backend_ref_mgr
            .get_refs(target_key)
            .into_iter()
            .filter(|resource_ref| {
                matches!(
                    resource_ref.kind,
                    ResourceKind::BackendTLSPolicy | ResourceKind::EdgionBackendTrafficPolicy
                )
            })
        {
            tracing::debug!(
                policy = %policy_ref.resource_key(),
                backend = %target_key,
                "Requeuing EdgionBackendTrafficPolicy due to route backend target change"
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
    for service_key in dropped_service_keys {
        for btp_ref in btp_refs_for_service(&ctx.runtime().service_ref_mgr, service_key) {
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
    use crate::conf_mgr::sync_runtime::resource_processor::BidirectionalRefManager;

    fn btp_ref(ns: &str, name: &str) -> ResourceRef {
        ResourceRef::new(ResourceKind::BackendTLSPolicy, Some(ns.to_string()), name.to_string())
    }

    /// `btp_refs_for_service` returns the BackendTLSPolicies registered under a
    /// Service key and nothing else. Routes and EBTPs register under the same
    /// keys, so the kind filter is what keeps a route from requeuing itself.
    #[test]
    fn btp_refs_for_service_returns_only_backend_tls_policies() {
        let mgr: SecretRefManager = BidirectionalRefManager::with_component("test");

        let policy = btp_ref("ns", "btp-a");
        let other_policy = btp_ref("ns", "btp-b");
        mgr.add_ref("ns/svc".to_string(), policy.clone());
        mgr.add_ref("ns/svc".to_string(), other_policy.clone());
        mgr.add_ref(
            "ns/svc".to_string(),
            ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "route".to_string()),
        );
        mgr.add_ref(
            "ns/svc".to_string(),
            ResourceRef::new(
                ResourceKind::EdgionBackendTrafficPolicy,
                Some("ns".to_string()),
                "ebtp".to_string(),
            ),
        );

        let mut got: Vec<String> = btp_refs_for_service(&mgr, "ns/svc")
            .iter()
            .map(|r| r.resource_key())
            .collect();
        got.sort();

        assert_eq!(got, vec!["ns/btp-a", "ns/btp-b"]);
    }

    /// A Service with no BTP targets yields no refs, and a same-prefix Service
    /// name must not over-match.
    #[test]
    fn btp_refs_for_service_no_match_is_empty() {
        let mgr: SecretRefManager = BidirectionalRefManager::with_component("test");
        mgr.add_ref("ns/svc".to_string(), btp_ref("ns", "btp-a"));

        assert!(btp_refs_for_service(&mgr, "ns/nomatch").is_empty());
        assert!(btp_refs_for_service(&mgr, "ns/svc-other").is_empty());
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
    fn typed_ancestor_resolver_separates_same_named_service_and_backend() {
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
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "ns".to_string(),
            name: "shared".to_string(),
        };
        runtime
            .service_ref_mgr
            .add_ref("ns/shared".to_string(), service_route.clone());
        runtime.backend_ref_mgr.add_ref(ai_target.to_string(), ai_route.clone());

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
