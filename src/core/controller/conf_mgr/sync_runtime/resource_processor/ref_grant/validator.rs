//! Cross-namespace reference validator
//!
//! Validates whether cross-namespace references are allowed by ReferenceGrants

use std::sync::Arc;

use super::ReferenceGrantStore;
use crate::core::common::config::is_reference_grant_validation_enabled;
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::types::resources::{gateway::SecretObjectReference, http_route::BackendObjectReference};

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

/// Check if ReferenceGrant validation is enabled by querying EdgionGatewayConfig
fn is_validation_enabled() -> bool {
    is_reference_grant_validation_enabled()
}

fn normalize_group(group: Option<&String>) -> &str {
    group.map(String::as_str).unwrap_or("")
}

fn normalize_kind(kind: Option<&String>) -> &str {
    kind.map(String::as_str).unwrap_or("Service")
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

/// Validate HTTPRoute with ReferenceGrant if validation is enabled.
///
/// Cross-namespace backend ref violations are NOT returned as validation errors here.
/// They are handled by parse() (sets ref_denied on each backend) and update_status()
/// (sets ResolvedRefs=False). Returning them here would cause Accepted=False, which
/// violates Gateway API spec — ref_denied routes must remain Accepted=True.
pub fn validate_http_route_if_enabled(route: &crate::types::resources::HTTPRoute) -> Vec<String> {
    let mut errors = Vec::new();
    let rules = route.spec.rules.as_deref().unwrap_or_default();
    for rule in rules {
        let matches = rule.matches.as_deref().unwrap_or_default();
        for m in matches {
            for h in m.headers.as_deref().unwrap_or_default() {
                let mt = h.match_type.as_deref().unwrap_or("Exact");
                if mt != "Exact" && mt != "RegularExpression" {
                    tracing::warn!(
                        ns = route.metadata.namespace.as_deref().unwrap_or(""),
                        name = route.metadata.name.as_deref().unwrap_or(""),
                        kind = "HTTPRoute",
                        field = "match_type",
                        value = %mt,
                        "Unsupported enum value, rejecting resource"
                    );
                    errors.push(format!(
                        "Unsupported HTTPRoute header match type '{}' (allowed: Exact, RegularExpression)",
                        mt
                    ));
                }
            }
            for q in m.query_params.as_deref().unwrap_or_default() {
                let mt = q.match_type.as_deref().unwrap_or("Exact");
                if mt != "Exact" && mt != "RegularExpression" {
                    tracing::warn!(
                        ns = route.metadata.namespace.as_deref().unwrap_or(""),
                        name = route.metadata.name.as_deref().unwrap_or(""),
                        kind = "HTTPRoute",
                        field = "match_type",
                        value = %mt,
                        "Unsupported enum value, rejecting resource"
                    );
                    errors.push(format!(
                        "Unsupported HTTPRoute query param match type '{}' (allowed: Exact, RegularExpression)",
                        mt
                    ));
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
pub fn validate_grpc_route_if_enabled(route: &crate::types::resources::GRPCRoute) -> Vec<String> {
    let mut errors = Vec::new();
    let rules = route.spec.rules.as_deref().unwrap_or_default();
    for rule in rules {
        let matches = rule.matches.as_deref().unwrap_or_default();
        for m in matches {
            for h in m.headers.as_deref().unwrap_or_default() {
                let mt = h.match_type.as_deref().unwrap_or("Exact");
                if mt != "Exact" && mt != "RegularExpression" {
                    tracing::warn!(
                        ns = route.metadata.namespace.as_deref().unwrap_or(""),
                        name = route.metadata.name.as_deref().unwrap_or(""),
                        kind = "GRPCRoute",
                        field = "match_type",
                        value = %mt,
                        "Unsupported enum value, rejecting resource"
                    );
                    errors.push(format!(
                        "Unsupported GRPCRoute header match type '{}' (allowed: Exact, RegularExpression)",
                        mt
                    ));
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
pub fn validate_tcp_route_if_enabled(_route: &crate::types::resources::TCPRoute) -> Vec<String> {
    Vec::new()
}

/// Validate UDPRoute with ReferenceGrant if validation is enabled.
///
/// Cross-namespace backend ref violations are handled by parse() (sets ref_denied)
/// and update_status() (sets ResolvedRefs=False), not here.
pub fn validate_udp_route_if_enabled(_route: &crate::types::resources::UDPRoute) -> Vec<String> {
    Vec::new()
}

/// Validate TLSRoute with ReferenceGrant if validation is enabled.
///
/// Cross-namespace backend ref violations are handled by parse() (sets ref_denied)
/// and update_status() (sets ResolvedRefs=False), not here.
pub fn validate_tls_route_if_enabled(_route: &crate::types::resources::TLSRoute) -> Vec<String> {
    Vec::new()
}
