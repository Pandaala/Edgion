//! Cross-namespace reference validator
//!
//! Validates whether cross-namespace references are allowed by ReferenceGrants

use super::ReferenceGrantStore;
use crate::types::resources::{gateway::SecretObjectReference, http_route::BackendObjectReference};
use std::sync::Arc;

/// Validator for cross-namespace references
pub struct CrossNamespaceValidator {
    pub(crate) store: Arc<ReferenceGrantStore>,
}

impl CrossNamespaceValidator {
    pub fn new() -> Self {
        Self {
            store: super::get_global_reference_grant_store(),
        }
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

impl Default for CrossNamespaceValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if ReferenceGrant validation is enabled by querying EdgionGatewayConfig
fn is_validation_enabled() -> bool {
    use crate::core::gateway::edgion_gateway_config::list_edgion_gateway_configs;
    list_edgion_gateway_configs()
        .iter()
        .any(|egwc| egwc.spec.enable_reference_grant_validation)
}

fn normalize_group(group: Option<&String>) -> &str {
    group.map(String::as_str).unwrap_or("")
}

fn normalize_kind(kind: Option<&String>) -> &str {
    kind.map(String::as_str).unwrap_or("Service")
}

fn check_cross_namespace_ref(
    store: &ReferenceGrantStore,
    route_namespace: &str,
    route_kind: &str,
    backend_ns: &str,
    group: &str,
    kind: &str,
    name: &str,
) -> Option<String> {
    let allowed = store.check_reference_allowed(
        route_namespace,
        "gateway.networking.k8s.io",
        route_kind,
        backend_ns,
        group,
        kind,
        Some(name),
    );

    if allowed {
        None
    } else {
        Some(format!(
            "Cross-namespace reference not allowed: {} in namespace '{}' cannot reference {}/{}/{} in namespace '{}' (no ReferenceGrant)",
            route_kind,
            route_namespace,
            if group.is_empty() { "core" } else { group },
            kind,
            name,
            backend_ns,
        ))
    }
}

/// Validate HTTPRoute with ReferenceGrant if validation is enabled
pub fn validate_http_route_if_enabled(route: &crate::types::resources::HTTPRoute) -> Vec<String> {
    if !is_validation_enabled() {
        return Vec::new();
    }

    let validator = CrossNamespaceValidator::new();
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");

    let mut errors = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                // HTTPBackendRef is a simpler type, convert to BackendObjectReference for validation
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != route_namespace {
                            let group = normalize_group(backend_ref.group.as_ref());
                            let kind = normalize_kind(backend_ref.kind.as_ref());
                            if let Some(err) = check_cross_namespace_ref(
                                &validator.store,
                                route_namespace,
                                "HTTPRoute",
                                backend_ns,
                                group,
                                kind,
                                &backend_ref.name,
                            ) {
                                errors.push(err);
                            }
                        }
                    }
                }
            }
        }
    }
    errors
}

/// Validate GRPCRoute with ReferenceGrant if validation is enabled
pub fn validate_grpc_route_if_enabled(route: &crate::types::resources::GRPCRoute) -> Vec<String> {
    if !is_validation_enabled() {
        return Vec::new();
    }

    let validator = CrossNamespaceValidator::new();
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");

    let mut errors = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                // GRPCBackendRef is also simpler, convert for validation
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != route_namespace {
                            let group = normalize_group(backend_ref.group.as_ref());
                            let kind = normalize_kind(backend_ref.kind.as_ref());
                            if let Some(err) = check_cross_namespace_ref(
                                &validator.store,
                                route_namespace,
                                "GRPCRoute",
                                backend_ns,
                                group,
                                kind,
                                &backend_ref.name,
                            ) {
                                errors.push(err);
                            }
                        }
                    }
                }
            }
        }
    }
    errors
}

/// Validate TCPRoute with ReferenceGrant if validation is enabled
pub fn validate_tcp_route_if_enabled(route: &crate::types::resources::TCPRoute) -> Vec<String> {
    if !is_validation_enabled() {
        return Vec::new();
    }

    let validator = CrossNamespaceValidator::new();
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");

    let mut errors = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != route_namespace {
                            let group = normalize_group(backend_ref.group.as_ref());
                            let kind = normalize_kind(backend_ref.kind.as_ref());
                            if let Some(err) = check_cross_namespace_ref(
                                &validator.store,
                                route_namespace,
                                "TCPRoute",
                                backend_ns,
                                group,
                                kind,
                                &backend_ref.name,
                            ) {
                                errors.push(err);
                            }
                        }
                    }
                }
            }
        }
    }
    errors
}

/// Validate UDPRoute with ReferenceGrant if validation is enabled
pub fn validate_udp_route_if_enabled(route: &crate::types::resources::UDPRoute) -> Vec<String> {
    if !is_validation_enabled() {
        return Vec::new();
    }

    let validator = CrossNamespaceValidator::new();
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");

    let mut errors = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != route_namespace {
                            let group = normalize_group(backend_ref.group.as_ref());
                            let kind = normalize_kind(backend_ref.kind.as_ref());
                            if let Some(err) = check_cross_namespace_ref(
                                &validator.store,
                                route_namespace,
                                "UDPRoute",
                                backend_ns,
                                group,
                                kind,
                                &backend_ref.name,
                            ) {
                                errors.push(err);
                            }
                        }
                    }
                }
            }
        }
    }
    errors
}

/// Validate TLSRoute with ReferenceGrant if validation is enabled
pub fn validate_tls_route_if_enabled(route: &crate::types::resources::TLSRoute) -> Vec<String> {
    if !is_validation_enabled() {
        return Vec::new();
    }

    let validator = CrossNamespaceValidator::new();
    let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");

    let mut errors = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != route_namespace {
                            let group = normalize_group(backend_ref.group.as_ref());
                            let kind = normalize_kind(backend_ref.kind.as_ref());
                            if let Some(err) = check_cross_namespace_ref(
                                &validator.store,
                                route_namespace,
                                "TLSRoute",
                                backend_ns,
                                group,
                                kind,
                                &backend_ref.name,
                            ) {
                                errors.push(err);
                            }
                        }
                    }
                }
            }
        }
    }
    errors
}
