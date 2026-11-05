//! Shared HTTPRoute/GRPCRoute delegation vocabulary.
//!
//! Pure, runtime-independent helpers consumed by the Controller (validation,
//! indexing) and the Gateway (slot table): parentRef shape classification,
//! delegating-rule shape checks, the delegation key, and the standard-filter
//! bitmask used for mandatory-filter conflict detection.

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use super::common::ParentReference;
use super::grpc_route::{GRPCMethodMatchType, GRPCRouteFilter, GRPCRouteFilterType, GRPCRouteRule};
use super::http_route::{HTTPRouteFilter, HTTPRouteFilterType, HTTPRouteRule, GATEWAY_API_GROUP};

/// Maximum delegation levels (root + 4 delegation edges).
pub const MAX_ROUTE_DELEGATION_LEVELS: usize = 5;
/// Maximum route-kind parentRefs a child route may declare.
pub const MAX_ROUTE_KIND_PARENT_REFS: usize = 3;

/// Wildcard spelling of a delegation target name.
const WILDCARD: &str = "*";

/// Route kind taking part in delegation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RouteKindName {
    HTTPRoute,
    GRPCRoute,
}

impl RouteKindName {
    pub fn as_str(&self) -> &'static str {
        match self {
            RouteKindName::HTTPRoute => "HTTPRoute",
            RouteKindName::GRPCRoute => "GRPCRoute",
        }
    }
}

/// Target name of a delegation edge: a concrete route or every route that
/// declares the delegating parent.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DelegationTargetName {
    Named(String),
    Wildcard,
}

impl DelegationTargetName {
    fn from_name(name: &str) -> Self {
        if name == WILDCARD {
            DelegationTargetName::Wildcard
        } else {
            DelegationTargetName::Named(name.to_string())
        }
    }
}

impl Serialize for DelegationTargetName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            DelegationTargetName::Named(n) => serializer.serialize_str(n),
            DelegationTargetName::Wildcard => serializer.serialize_str(WILDCARD),
        }
    }
}

impl<'de> Deserialize<'de> for DelegationTargetName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(DelegationTargetName::from_name(&s))
    }
}

/// Identity of one delegation target of a parent route.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DelegationKey {
    pub kind: RouteKindName,
    pub parent_ns: String,
    pub parent_name: String,
    pub target_ns: String,
    pub target_name: DelegationTargetName,
}

/// Delegation target as written by a delegating rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationTarget {
    pub ns: String,
    pub name: DelegationTargetName,
}

/// Build the key for a (parent, target) pair.
pub fn key_for(kind: RouteKindName, parent_ns: &str, parent_name: &str, target: &DelegationTarget) -> DelegationKey {
    DelegationKey {
        kind,
        parent_ns: parent_ns.to_string(),
        parent_name: parent_name.to_string(),
        target_ns: target.ns.clone(),
        target_name: target.name.clone(),
    }
}

/// Shape of a route's parentRefs with respect to delegation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentRefShape {
    /// No parentRefs at all.
    None,
    /// No route-kind parentRefs: today's per-parentRef behavior.
    Root,
    /// 1..=3 parentRefs of the route's own kind, as (namespace, name).
    Child(Vec<(String, String)>),
    /// A route-kind parentRef is present but the shape is illegal.
    Invalid(&'static str),
}

fn is_route_kind(kind: &str) -> bool {
    kind == "HTTPRoute" || kind == "GRPCRoute"
}

/// Classify parentRefs per the exclusive-shape rule.
pub fn classify_parent_refs(refs: Option<&[ParentReference]>, route_ns: &str, kind: RouteKindName) -> ParentRefShape {
    let refs = match refs {
        Some(r) if !r.is_empty() => r,
        _ => return ParentRefShape::None,
    };
    let is_route_ref = |p: &ParentReference| {
        p.group.as_deref().unwrap_or(GATEWAY_API_GROUP) == GATEWAY_API_GROUP
            && p.kind.as_deref().is_some_and(is_route_kind)
    };
    let route_ref_count = refs.iter().filter(|p| is_route_ref(p)).count();
    if route_ref_count == 0 {
        return ParentRefShape::Root;
    }
    if route_ref_count != refs.len() {
        return ParentRefShape::Invalid("parentRefs must be either all Gateway refs or all route refs, not a mix");
    }
    if refs.iter().any(|p| p.kind.as_deref() != Some(kind.as_str())) {
        return ParentRefShape::Invalid("route parentRefs must reference the same route kind as this route");
    }
    if refs.len() > MAX_ROUTE_KIND_PARENT_REFS {
        return ParentRefShape::Invalid("a child route may declare at most 3 route parentRefs");
    }
    ParentRefShape::Child(
        refs.iter()
            .map(|p| {
                (
                    p.namespace.clone().unwrap_or_else(|| route_ns.to_string()),
                    p.name.clone(),
                )
            })
            .collect(),
    )
}

/// Shared checks on the single delegating backendRef.
fn check_backend_ref(count: usize, port: Option<i32>, has_filters: bool) -> Result<(), String> {
    if count != 1 {
        return Err(format!(
            "a delegating rule must have exactly one backendRef (found {count})"
        ));
    }
    if port.is_some() {
        return Err("delegation backendRef must not set 'port'".to_string());
    }
    if has_filters {
        return Err("delegation backendRef must not carry per-backend filters".to_string());
    }
    Ok(())
}

/// Delegation target of an HTTP rule. `None`: the rule does not delegate.
/// `Err`: the rule delegates but is malformed.
pub fn delegation_target_of_http_rule(
    rule: &HTTPRouteRule,
    route_ns: &str,
) -> Option<Result<DelegationTarget, String>> {
    let refs = rule.backend_refs.as_ref()?;
    if !refs.iter().any(|b| b.is_route_delegation_ref()) {
        return None;
    }
    Some((|| {
        check_backend_ref(refs.len(), refs[0].port, refs[0].filters.is_some())?;
        for m in rule.matches.iter().flatten() {
            let match_type = m
                .path
                .as_ref()
                .and_then(|p| p.match_type.as_deref())
                .unwrap_or("PathPrefix");
            if match_type != "PathPrefix" {
                return Err(format!(
                    "delegating rule matches must use PathPrefix (found {match_type})"
                ));
            }
        }
        Ok(DelegationTarget {
            ns: refs[0].namespace.clone().unwrap_or_else(|| route_ns.to_string()),
            name: DelegationTargetName::from_name(&refs[0].name),
        })
    })())
}

/// Delegation target of a gRPC rule (service-level delegation only).
pub fn delegation_target_of_grpc_rule(
    rule: &GRPCRouteRule,
    route_ns: &str,
) -> Option<Result<DelegationTarget, String>> {
    let refs = rule.backend_refs.as_ref()?;
    if !refs.iter().any(|b| b.is_route_delegation_ref()) {
        return None;
    }
    Some((|| {
        check_backend_ref(refs.len(), refs[0].port, refs[0].filters.is_some())?;
        let matches = match rule.matches.as_deref() {
            Some(m) if !m.is_empty() => m,
            _ => {
                return Err("a delegating rule must set at least one match (delegate-all is not supported)".to_string())
            }
        };
        for m in matches {
            let Some(method) = m.method.as_ref() else {
                return Err(
                    "delegating rule matches must set a method matcher with 'service' (found a header-only match)"
                        .to_string(),
                );
            };
            if !matches!(method.match_type, None | Some(GRPCMethodMatchType::Exact)) {
                return Err(format!(
                    "delegating rule matches must use Exact (or omitted) method matching (found {:?})",
                    method.match_type
                ));
            }
            if method.service.as_deref().is_none_or(str::is_empty) {
                return Err("delegating rule matches must set a non-empty 'method.service'".to_string());
            }
            if method.method.is_some() {
                return Err(
                    "delegating rule matches must not set 'method.method' (service-level delegation only)".to_string(),
                );
            }
        }
        Ok(DelegationTarget {
            ns: refs[0].namespace.clone().unwrap_or_else(|| route_ns.to_string()),
            name: DelegationTargetName::from_name(&refs[0].name),
        })
    })())
}

/// Bit of one standard HTTP filter type; `ExtensionRef` contributes none.
fn http_filter_bit(f: &HTTPRouteFilter) -> u32 {
    match f.filter_type {
        HTTPRouteFilterType::RequestHeaderModifier => 1 << 0,
        HTTPRouteFilterType::ResponseHeaderModifier => 1 << 1,
        HTTPRouteFilterType::RequestRedirect => 1 << 2,
        HTTPRouteFilterType::URLRewrite => 1 << 3,
        HTTPRouteFilterType::RequestMirror => 1 << 4,
        HTTPRouteFilterType::CORS => 1 << 5,
        HTTPRouteFilterType::ExternalAuth => 1 << 6,
        HTTPRouteFilterType::ExtensionRef => 0,
    }
}

/// Bit of one standard gRPC filter type; `ExtensionRef` contributes none.
fn grpc_filter_bit(f: &GRPCRouteFilter) -> u32 {
    match f.filter_type {
        GRPCRouteFilterType::RequestHeaderModifier => 1 << 0,
        GRPCRouteFilterType::ResponseHeaderModifier => 1 << 1,
        GRPCRouteFilterType::ExtensionRef => 0,
    }
}

/// Standard HTTP filter types this rule applies anywhere: one bit per type
/// across the rule's own `filters` and every `backendRefs[*].filters`.
/// Backend-level filters run after the route chain, so leaving them out
/// would let a child override a parent's mandatory filter unnoticed.
/// `ExtensionRef` is excluded.
pub fn standard_filter_mask_http(rule: &HTTPRouteRule) -> u32 {
    let backend_filters = rule
        .backend_refs
        .iter()
        .flatten()
        .flat_map(|b| b.filters.iter().flatten());
    rule.filters
        .iter()
        .flatten()
        .chain(backend_filters)
        .fold(0, |mask, f| mask | http_filter_bit(f))
}

/// Standard gRPC filter types this rule applies anywhere (rule-level and
/// backendRef-level filters); see [`standard_filter_mask_http`].
pub fn standard_filter_mask_grpc(rule: &GRPCRouteRule) -> u32 {
    let backend_filters = rule
        .backend_refs
        .iter()
        .flatten()
        .flat_map(|b| b.filters.iter().flatten());
    rule.filters
        .iter()
        .flatten()
        .chain(backend_filters)
        .fold(0, |mask, f| mask | grpc_filter_bit(f))
}

#[cfg(test)]
#[path = "../../tests/unit/resources/route_delegation_test.rs"]
mod tests;
