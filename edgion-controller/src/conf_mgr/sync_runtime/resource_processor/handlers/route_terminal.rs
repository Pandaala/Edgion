//! Terminal rule identity and root authentication resolution for HTTPRoute.
//!
//! Both run for every HTTPRoute, delegating or not: terminal identities are
//! read by AI routing, and root auth decisions stamp `ExternalAuth` filters and
//! feed the route's `ResolvedRefs` status.

use super::route_auth::{self, AuthDecision};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError};
use edgion_resources::resources::http_route::{HTTPRoute, HTTPRouteRule};
use edgion_resources::resources::refs::route_fields::HttpRouteBackendRefs;

const HTTP_ROUTE_KIND: &str = "HTTPRoute";

/// Replace every operator-provided or stale terminal identity on the route's
/// own rules with controller-derived values.
pub(super) fn populate_terminal_rule_identities(route: &mut HTTPRoute) {
    let route_identity = terminal_route_identity(route);
    for (index, rule) in route.spec.rules.iter_mut().flatten().enumerate() {
        set_terminal_rule_identity(rule, &route_identity, index);
    }
}

fn terminal_route_identity(route: &HTTPRoute) -> String {
    route.metadata.uid.clone().unwrap_or_else(|| {
        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let name = route.metadata.name.as_deref().unwrap_or("");
        format!("{HTTP_ROUTE_KIND}/{namespace}/{name}")
    })
}

fn set_terminal_rule_identity(rule: &mut HTTPRouteRule, route_identity: &str, index: usize) {
    rule.resolved_terminal_route_uid = Some(route_identity.to_string());
    rule.resolved_terminal_rule_identity = Some(match rule.name.as_deref() {
        Some(name) => format!("name:{name}"),
        None => format!("index:{index}"),
    });
}

/// Auth decisions for the route's own authored auth fields.
pub(super) struct RootAuth {
    pub(super) decisions: Vec<AuthDecision>,
    pub(super) errors: Vec<ResolvedRefsError>,
}

/// Resolve the route's own auth fields. `register` records recovery
/// dependencies (parse); status passes `false`. An invalid reference source or
/// a route without rules yields no decisions.
pub(super) fn root_auth(route: &HTTPRoute, ctx: &HandlerContext, register: bool) -> RootAuth {
    if HttpRouteBackendRefs::from_route(route).is_err() || route.spec.rules.is_none() {
        return RootAuth {
            decisions: Vec::new(),
            errors: Vec::new(),
        };
    }
    let decisions = route_auth::decisions(route, |field| route_auth::resolve(route, field, ctx, register));
    let errors = decisions
        .iter()
        .flat_map(|decision| decision.errors.iter().cloned())
        .collect();
    RootAuth { decisions, errors }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/route_terminal_test.rs"]
mod tests;
