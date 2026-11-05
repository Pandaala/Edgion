//! GRPCRoute delegation validation facts.
//!
//! A parent GRPCRoute rule delegates a gRPC service to child GRPCRoutes via a
//! single `backendRef {group: gateway.networking.k8s.io, kind: GRPCRoute}`
//! (named, or `name: "*"` for every declaring child in the target namespace).
//! As for HTTPRoute, the Controller no longer flattens delegation trees: each
//! route publishes its own rules and each child carries the controller-resolved
//! `resolved_delegation_keys` of its confirmed edges. The graph sync and the
//! status helpers are shared with HTTPRoute in `route_delegation.rs`.
//!
//! Delegating-rule shape (design §9): every match has `method.type: Exact`
//! (or omitted) with a non-empty `service` and no `method`; a matchless
//! delegating rule is invalid.

use std::collections::HashSet;

use super::super::delegation_graph::DelegatingRule;
use super::route_delegation::{backend_selecting_plugin_of, ChildMatch, ChildRuleFacts, GrpcDelegationFacts};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::grpc_route::{GRPCMethodMatchType, GRPCRoute, GRPCRouteRule};
use edgion_resources::resources::refs::route_fields::GrpcRouteExtensionRefs;
use edgion_resources::resources::route_delegation::{
    delegation_target_of_grpc_rule, standard_filter_mask_grpc, RouteKindName,
};

/// Compute the route's delegation facts. An ordinary route (no delegating
/// rule, no route-kind parentRef) costs one parentRef classification and one
/// rule scan, and never reads the runtime.
pub(super) fn grpc_delegation_facts(route: &GRPCRoute, runtime: &ControllerRuntime) -> GrpcDelegationFacts {
    let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
    let mut facts = GrpcDelegationFacts::new(
        route.spec.parent_refs.as_deref(),
        route_ns,
        RouteKindName::GRPCRoute,
        route.spec.hostnames.as_ref(),
    );
    let inert = facts.inert;
    let Some(rules) = route.spec.rules.as_ref() else {
        return facts;
    };

    let mut statically_invalid: Option<HashSet<usize>> = None;
    for (index, rule) in rules.iter().enumerate() {
        let target = match delegation_target_of_grpc_rule(rule, route_ns) {
            None => continue,
            Some(Err(reason)) => {
                facts.malformed_rules.push((index, reason));
                continue;
            }
            Some(Ok(target)) => target,
        };
        let backend = &rule.backend_refs.as_ref().expect("a delegating rule has a backendRef")[0];
        if let Err(error) = backend.validated_target(route_ns) {
            facts
                .malformed_rules
                .push((index, format!("Invalid delegation reference: {error}")));
            continue;
        }
        if let Some(plugin) = backend_selecting_plugin(route, index, runtime) {
            facts.backend_plugin_on_delegating_rule.push((
                index,
                format!("{plugin} selects the upstream and is valid only on terminal rules, not on a GRPCRoute delegation rule"),
            ));
            continue;
        }
        let invalid = statically_invalid.get_or_insert_with(|| {
            super::grpc_route::find_invalid_rule_indices(route, route_ns)
                .into_iter()
                .collect()
        });
        if inert || invalid.contains(&index) {
            continue;
        }
        facts.add_target(target, grpc_delegating_rule(rule));
    }
    facts
}

/// Delegating-rule facts of a valid gRPC delegating rule: its mask and the
/// delegated services (every match of a valid delegating rule names one).
fn grpc_delegating_rule(rule: &GRPCRouteRule) -> DelegatingRule {
    let scopes = rule
        .matches
        .iter()
        .flatten()
        .map(|m| m.method.as_ref().and_then(|method| method.service.clone()))
        .collect::<Option<Vec<String>>>();
    DelegatingRule {
        mask: standard_filter_mask_grpc(rule),
        scopes: scopes.filter(|services| !services.is_empty()),
    }
}

/// Static facts of every gRPC child rule, in rule order. Only an `Exact`
/// (or omitted-type) method match naming a service can be checked; a
/// regular expression, a match without a service and a header-only match
/// are assumed to be reachable.
pub(super) fn grpc_child_rules(rules: Option<&Vec<GRPCRouteRule>>) -> Vec<ChildRuleFacts> {
    rules
        .into_iter()
        .flatten()
        .map(|rule| ChildRuleFacts {
            mask: standard_filter_mask_grpc(rule),
            matches: rule
                .matches
                .iter()
                .flatten()
                .map(|m| match m.method.as_ref() {
                    Some(method) if matches!(method.match_type, None | Some(GRPCMethodMatchType::Exact)) => method
                        .service
                        .clone()
                        .filter(|service| !service.is_empty())
                        .map_or(ChildMatch::Unchecked, ChildMatch::GrpcService),
                    _ => ChildMatch::Unchecked,
                })
                .collect(),
        })
        .collect()
}

/// First enabled backend-selecting request plugin referenced by a rule-level
/// EdgionPlugins ExtensionRef of rule `index`.
fn backend_selecting_plugin(route: &GRPCRoute, index: usize, runtime: &ControllerRuntime) -> Option<&'static str> {
    let references = GrpcRouteExtensionRefs::from_route(route).ok()?;
    references
        .authored()
        .filter(|field| field.slot().rule() == index && field.slot().backend().is_none())
        .find_map(|field| {
            let reference = field.validated().ok()??;
            let target = reference.target();
            let key = format!("{}/{}", target.scope().namespace()?, target.name());
            backend_selecting_plugin_of(runtime, &key)
        })
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route_delegation_test.rs"]
mod tests;
