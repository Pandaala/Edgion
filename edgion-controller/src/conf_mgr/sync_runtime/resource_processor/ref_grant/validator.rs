//! Cross-namespace reference validator
//!
//! Validates whether cross-namespace references are allowed by ReferenceGrants

use crate::conf_mgr::sync_runtime::resource_processor::ref_manager::ResourceRef;
use crate::conf_mgr::ControllerRuntime;

/// Register a cross-namespace dependency before reading the authorization
/// source. This ordering ensures a concurrent ReferenceGrant change either
/// precedes `read` or observes `owner` and requeues it afterwards.
pub(crate) fn read_after_registering_cross_ns_dependency<T>(
    runtime: &ControllerRuntime,
    target_namespace: &str,
    owner: &ResourceRef,
    read: impl FnOnce() -> T,
) -> T {
    runtime
        .cross_ns_ref_mgr
        .add_ref(target_namespace.to_string(), owner.clone());
    read()
}

/// Try compile a regex pattern; on failure push a structured validation error.
fn check_regex_pattern(pattern: &str, kind: &str, field: &str, ns: &str, name: &str, errors: &mut Vec<String>) {
    if let Err(e) = regex::Regex::new(pattern) {
        tracing::warn!(
            ns = ns,
            name = name,
            kind = kind,
            field = field,
            pattern = %pattern,
            error = %e,
            "Invalid RegularExpression pattern, rejecting resource"
        );
        errors.push(format!("Invalid {} {} regex '{}': {}", kind, field, pattern, e));
    }
}

/// Validate HTTPRoute with ReferenceGrant if validation is enabled.
///
/// Cross-namespace backend ref violations are NOT returned as validation errors here.
/// They are handled by parse() (sets ref_denied on each backend) and update_status()
/// (sets ResolvedRefs=False). Returning them here would cause Accepted=False, which
/// violates Gateway API spec — ref_denied routes must remain Accepted=True.
pub fn validate_http_route_if_enabled(route: &edgion_resources::resources::HTTPRoute) -> Vec<String> {
    let mut errors = Vec::new();
    let ns = route.metadata.namespace.as_deref().unwrap_or("");
    let name = route.metadata.name.as_deref().unwrap_or("");
    let rules = route.spec.rules.as_deref().unwrap_or_default();
    for rule in rules {
        let matches = rule.matches.as_deref().unwrap_or_default();
        for m in matches {
            if let Some(p) = &m.path {
                if p.match_type.as_deref() == Some("RegularExpression") {
                    if let Some(value) = p.value.as_deref() {
                        check_regex_pattern(value, "HTTPRoute", "path", ns, name, &mut errors);
                    }
                }
            }
            for h in m.headers.as_deref().unwrap_or_default() {
                let mt = h.match_type.as_deref().unwrap_or("Exact");
                if mt != "Exact" && mt != "RegularExpression" {
                    tracing::warn!(
                        ns = ns,
                        name = name,
                        kind = "HTTPRoute",
                        field = "match_type",
                        value = %mt,
                        "Unsupported enum value, rejecting resource"
                    );
                    errors.push(format!(
                        "Unsupported HTTPRoute header match type '{}' (allowed: Exact, RegularExpression)",
                        mt
                    ));
                } else if mt == "RegularExpression" {
                    check_regex_pattern(&h.value, "HTTPRoute", "header", ns, name, &mut errors);
                }
            }
            for q in m.query_params.as_deref().unwrap_or_default() {
                let mt = q.match_type.as_deref().unwrap_or("Exact");
                if mt != "Exact" && mt != "RegularExpression" {
                    tracing::warn!(
                        ns = ns,
                        name = name,
                        kind = "HTTPRoute",
                        field = "match_type",
                        value = %mt,
                        "Unsupported enum value, rejecting resource"
                    );
                    errors.push(format!(
                        "Unsupported HTTPRoute query param match type '{}' (allowed: Exact, RegularExpression)",
                        mt
                    ));
                } else if mt == "RegularExpression" {
                    check_regex_pattern(&q.value, "HTTPRoute", "queryParam", ns, name, &mut errors);
                }
            }
        }
    }
    errors
}

/// Validate GRPCRoute with ReferenceGrant if validation is enabled.
///
/// Cross-namespace backend ref violations are NOT returned as validation errors here.
/// They are handled by parse() (sets ref_denied on each backend) and update_status()
/// (sets ResolvedRefs=False). Returning them here would cause Accepted=False, which
/// violates Gateway API spec — ref_denied routes must remain Accepted=True.
pub fn validate_grpc_route_if_enabled(route: &edgion_resources::resources::GRPCRoute) -> Vec<String> {
    let mut errors = Vec::new();
    let ns = route.metadata.namespace.as_deref().unwrap_or("");
    let name = route.metadata.name.as_deref().unwrap_or("");
    let rules = route.spec.rules.as_deref().unwrap_or_default();
    for rule in rules {
        let matches = rule.matches.as_deref().unwrap_or_default();
        for m in matches {
            // Method errors are per-rule, via GRPCRouteRule::method_match_error.
            // Returning them here would reject otherwise valid sibling rules.
            for h in m.headers.as_deref().unwrap_or_default() {
                let mt = h.match_type.as_deref().unwrap_or("Exact");
                if mt != "Exact" && mt != "RegularExpression" {
                    tracing::warn!(
                        ns = ns,
                        name = name,
                        kind = "GRPCRoute",
                        field = "match_type",
                        value = %mt,
                        "Unsupported enum value, rejecting resource"
                    );
                    errors.push(format!(
                        "Unsupported GRPCRoute header match type '{}' (allowed: Exact, RegularExpression)",
                        mt
                    ));
                } else if mt == "RegularExpression" {
                    check_regex_pattern(&h.value, "GRPCRoute", "header", ns, name, &mut errors);
                }
            }
        }
    }
    errors
}

/// Validate TCPRoute with ReferenceGrant if validation is enabled.
///
/// Cross-namespace backend ref violations are handled by parse() (sets ref_denied)
/// and update_status() (sets ResolvedRefs=False), not here.
pub fn validate_tcp_route_if_enabled(route: &edgion_resources::resources::TCPRoute) -> Vec<String> {
    if route.spec.rules.as_ref().is_some_and(|rules| rules.len() > 1) {
        vec!["TCPRoute v1 supports at most one rule".to_string()]
    } else {
        Vec::new()
    }
}

/// Validate UDPRoute with ReferenceGrant if validation is enabled.
///
/// Cross-namespace backend ref violations are handled by parse() (sets ref_denied)
/// and update_status() (sets ResolvedRefs=False), not here.
pub fn validate_udp_route_if_enabled(route: &edgion_resources::resources::UDPRoute) -> Vec<String> {
    if route.spec.rules.as_ref().is_some_and(|rules| rules.len() > 1) {
        vec!["UDPRoute v1 supports at most one rule".to_string()]
    } else {
        Vec::new()
    }
}

/// Validate TLSRoute with ReferenceGrant if validation is enabled.
///
/// Cross-namespace backend ref violations are handled by parse() (sets ref_denied)
/// and update_status() (sets ResolvedRefs=False), not here.
pub fn validate_tls_route_if_enabled(_route: &edgion_resources::resources::TLSRoute) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/ref_grant/validator_test.rs"]
mod tests;
