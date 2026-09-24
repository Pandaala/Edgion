//! Cross-namespace reference validator
//!
//! Validates whether cross-namespace references are allowed by ReferenceGrants

use std::sync::Arc;

use super::ReferenceGrantStore;
use crate::conf_mgr::sync_runtime::resource_processor::ref_manager::ResourceRef;
use crate::conf_mgr::ControllerRuntime;
use crate::config::is_reference_grant_validation_enabled;
use edgion_resources::resources::{gateway::SecretObjectReference, http_route::BackendObjectReference};

/// Validator for cross-namespace references
pub struct CrossNamespaceValidator {
    pub(crate) store: Arc<ReferenceGrantStore>,
}

impl CrossNamespaceValidator {
    pub fn from_store(store: Arc<ReferenceGrantStore>) -> Self {
        Self { store }
    }

    /// Validate Route's backendRefs for cross-namespace references
    ///
    /// # Arguments
    /// * `route_namespace` - Namespace of the route
    /// * `route_kind` - Kind of the route (e.g., "HTTPRoute", "TCPRoute")
    /// * `backend_refs` - Backend references to validate
    ///
    /// # Returns
    /// Vector of error messages for disallowed references
    pub fn validate_route_backend_refs(
        &self,
        route_namespace: &str,
        route_kind: &str,
        backend_refs: &[BackendObjectReference],
    ) -> Vec<String> {
        let mut errors = Vec::new();

        for backend_ref in backend_refs {
            if let Some(backend_ns) = &backend_ref.namespace {
                if backend_ns != route_namespace {
                    let group = if backend_ref.group.is_empty() {
                        ""
                    } else {
                        &backend_ref.group
                    };
                    let kind = backend_ref.kind.as_deref().unwrap_or("Service");

                    let allowed = self.store.check_reference_allowed(
                        route_namespace,
                        "gateway.networking.k8s.io",
                        route_kind,
                        backend_ns,
                        group,
                        kind,
                        Some(&backend_ref.name),
                    );

                    if !allowed {
                        errors.push(format!(
                            "Cross-namespace reference not allowed: {} in namespace '{}' cannot reference {}/{} in namespace '{}' (no ReferenceGrant)",
                            route_kind, route_namespace,
                            kind, backend_ref.name, backend_ns
                        ));
                    }
                }
            }
        }

        errors
    }

    /// Validate Gateway's certificateRefs for cross-namespace references
    ///
    /// # Arguments
    /// * `gateway_namespace` - Namespace of the gateway
    /// * `certificate_refs` - Certificate references to validate
    ///
    /// # Returns
    /// Vector of error messages for disallowed references
    pub fn validate_gateway_certificate_refs(
        &self,
        gateway_namespace: &str,
        certificate_refs: &[SecretObjectReference],
    ) -> Vec<String> {
        let mut errors = Vec::new();

        for cert_ref in certificate_refs {
            if let Some(cert_ns) = &cert_ref.namespace {
                if cert_ns != gateway_namespace {
                    let allowed = self.store.check_reference_allowed(
                        gateway_namespace,
                        "gateway.networking.k8s.io",
                        "Gateway",
                        cert_ns,
                        cert_ref.group.as_deref().unwrap_or(""),
                        cert_ref.kind.as_deref().unwrap_or("Secret"),
                        Some(&cert_ref.name),
                    );

                    if !allowed {
                        errors.push(format!(
                            "Cross-namespace reference not allowed: Gateway in namespace '{}' cannot reference Secret '{}' in namespace '{}' (no ReferenceGrant)",
                            gateway_namespace, cert_ref.name, cert_ns
                        ));
                    }
                }
            }
        }

        errors
    }
}

fn is_validation_enabled() -> bool {
    is_reference_grant_validation_enabled()
}

fn normalize_group(group: Option<&String>) -> &str {
    group.map(String::as_str).unwrap_or("")
}

fn normalize_kind(kind: Option<&String>) -> &str {
    kind.map(String::as_str).unwrap_or("Service")
}

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

/// Check if a single cross-namespace reference is allowed
/// Returns true if allowed, false otherwise
/// Used by handlers to set ref_denied field on BackendRef
pub fn is_cross_ns_ref_allowed(
    route_namespace: &str,
    route_kind: &str,
    backend_namespace: &str,
    backend_group: Option<&String>,
    backend_kind: Option<&String>,
    backend_name: &str,
    runtime: &ControllerRuntime,
) -> bool {
    if !is_validation_enabled() {
        return true;
    }

    let group = normalize_group(backend_group);
    let kind = normalize_kind(backend_kind);

    runtime.ref_grant_store.check_reference_allowed(
        route_namespace,
        "gateway.networking.k8s.io",
        route_kind,
        backend_namespace,
        group,
        kind,
        Some(backend_name),
    )
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
