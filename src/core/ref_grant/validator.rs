//! Cross-namespace reference validator
//!
//! Validates whether cross-namespace references are allowed by ReferenceGrants

use std::sync::Arc;
use crate::types::resources::{
    http_route::BackendObjectReference,
    gateway::SecretObjectReference,
};
use super::ReferenceGrantStore;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::{ReferenceGrant, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::HashMap;

    fn create_test_grant(
        namespace: &str,
        name: &str,
        from_namespace: &str,
        from_kind: &str,
        to_kind: &str,
    ) -> ReferenceGrant {
        ReferenceGrant {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantSpec {
                from: vec![ReferenceGrantFrom {
                    group: "gateway.networking.k8s.io".to_string(),
                    kind: from_kind.to_string(),
                    namespace: from_namespace.to_string(),
                }],
                to: vec![ReferenceGrantTo {
                    group: "".to_string(),
                    kind: to_kind.to_string(),
                    name: None,
                }],
            },
        }
    }

    #[test]
    fn test_validate_allowed_reference() {
        let store = super::super::get_global_reference_grant_store();
        
        // Setup: allow HTTPRoute from ns-source to access Service in ns-target
        let grant = create_test_grant("ns-target", "test-grant", "ns-source", "HTTPRoute", "Service");
        let mut grants = HashMap::new();
        grants.insert("ns-target/test-grant".to_string(), Arc::new(grant));
        store.replace_all(grants);

        let validator = CrossNamespaceValidator::new();
        
        let backend_refs = vec![BackendObjectReference {
            group: "".to_string(),
            kind: Some("Service".to_string()),
            name: "my-service".to_string(),
            namespace: Some("ns-target".to_string()),
            port: Some(80),
        }];

        let errors = validator.validate_route_backend_refs("ns-source", "HTTPRoute", &backend_refs);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_validate_disallowed_reference() {
        let store = super::super::get_global_reference_grant_store();
        store.replace_all(HashMap::new()); // Clear all grants

        let validator = CrossNamespaceValidator::new();
        
        let backend_refs = vec![BackendObjectReference {
            group: "".to_string(),
            kind: Some("Service".to_string()),
            name: "my-service".to_string(),
            namespace: Some("ns-target".to_string()),
            port: Some(80),
        }];

        let errors = validator.validate_route_backend_refs("ns-source", "HTTPRoute", &backend_refs);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("not allowed"));
    }
}

