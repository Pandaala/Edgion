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
pub fn validate_http_route_if_enabled(route: &crate::types::resources::HTTPRoute) -> Vec<String> {
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
pub fn validate_grpc_route_if_enabled(route: &crate::types::resources::GRPCRoute) -> Vec<String> {
    use crate::types::resources::grpc_route::GRPCMethodMatchType;
    let mut errors = Vec::new();
    let ns = route.metadata.namespace.as_deref().unwrap_or("");
    let name = route.metadata.name.as_deref().unwrap_or("");
    let rules = route.spec.rules.as_deref().unwrap_or_default();
    for rule in rules {
        let matches = rule.matches.as_deref().unwrap_or_default();
        for m in matches {
            if let Some(method) = &m.method {
                if method.match_type.as_ref() == Some(&GRPCMethodMatchType::RegularExpression) {
                    if let Some(svc) = method.service.as_deref() {
                        check_regex_pattern(svc, "GRPCRoute", "method.service", ns, name, &mut errors);
                    }
                    if let Some(mth) = method.method.as_deref() {
                        check_regex_pattern(mth, "GRPCRoute", "method.method", ns, name, &mut errors);
                    }
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::{ReferenceGrant, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::HashMap;

    // ---- Test helpers --------------------------------------------------------

    /// Build a ReferenceGrant with a single From and a single To entry.
    fn make_grant(
        ns: &str,
        name: &str,
        from_ns: &str,
        from_group: &str,
        from_kind: &str,
        to_group: &str,
        to_kind: &str,
        to_name: Option<&str>,
    ) -> ReferenceGrant {
        ReferenceGrant {
            metadata: ObjectMeta {
                namespace: Some(ns.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantSpec {
                from: vec![ReferenceGrantFrom {
                    group: from_group.to_string(),
                    kind: from_kind.to_string(),
                    namespace: from_ns.to_string(),
                }],
                to: vec![ReferenceGrantTo {
                    group: to_group.to_string(),
                    kind: to_kind.to_string(),
                    name: to_name.map(|s| s.to_string()),
                }],
            },
        }
    }

    fn store_with(grants: Vec<(String, ReferenceGrant)>) -> Arc<ReferenceGrantStore> {
        let store = ReferenceGrantStore::new();
        let mut map = HashMap::new();
        for (key, grant) in grants {
            map.insert(key, Arc::new(grant));
        }
        store.replace_all(map);
        Arc::new(store)
    }

    fn backend_ref(group: &str, kind: Option<&str>, name: &str, namespace: Option<&str>) -> BackendObjectReference {
        BackendObjectReference {
            group: group.to_string(),
            kind: kind.map(|s| s.to_string()),
            name: name.to_string(),
            namespace: namespace.map(|s| s.to_string()),
            port: Some(80),
        }
    }

    fn cert_ref(group: Option<&str>, kind: Option<&str>, name: &str, namespace: Option<&str>) -> SecretObjectReference {
        SecretObjectReference {
            group: group.map(|s| s.to_string()),
            kind: kind.map(|s| s.to_string()),
            name: name.to_string(),
            namespace: namespace.map(|s| s.to_string()),
        }
    }

    // ---- Route backend ref tests --------------------------------------------

    #[test]
    fn test_route_backend_same_namespace_is_always_allowed() {
        // No grants in store, but same-namespace refs must always pass.
        let store = store_with(vec![]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-route"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert!(
            errors.is_empty(),
            "same-namespace ref must be allowed, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_route_backend_no_namespace_field_is_allowed() {
        // namespace=None means "same namespace as the route", which is always allowed.
        let store = store_with(vec![]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "svc-a", None)];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert!(errors.is_empty());
    }

    #[test]
    fn test_route_backend_cross_ns_allowed_by_exact_grant() {
        let grant = make_grant(
            "ns-target",
            "g1",
            "ns-route",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            None,
        );
        let store = store_with(vec![("ns-target/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "any-svc", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert!(errors.is_empty(), "exact grant should allow ref, got: {:?}", errors);
    }

    #[test]
    fn test_route_backend_cross_ns_rejected_when_no_store_entries() {
        // Empty store: every cross-namespace ref must be rejected.
        let store = store_with(vec![]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Cross-namespace reference not allowed"));
        assert!(errors[0].contains("ns-route"));
        assert!(errors[0].contains("ns-target"));
        assert!(errors[0].contains("svc-a"));
    }

    #[test]
    fn test_route_backend_rejected_when_from_namespace_mismatch() {
        // Grant trusts "ns-other", but route is in "ns-route".
        let grant = make_grant(
            "ns-target",
            "g1",
            "ns-other",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            None,
        );
        let store = store_with(vec![("ns-target/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_route_backend_rejected_when_from_kind_mismatch() {
        // Grant trusts TCPRoute, but the caller is an HTTPRoute.
        let grant = make_grant(
            "ns-target",
            "g1",
            "ns-route",
            "gateway.networking.k8s.io",
            "TCPRoute",
            "",
            "Service",
            None,
        );
        let store = store_with(vec![("ns-target/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_route_backend_rejected_when_to_kind_mismatch() {
        // Grant trusts Secret, but the ref targets a Service.
        let grant = make_grant(
            "ns-target",
            "g1",
            "ns-route",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Secret",
            None,
        );
        let store = store_with(vec![("ns-target/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_route_backend_rejected_when_to_name_mismatch() {
        // Grant restricts to "allowed-svc", but ref is for "other-svc".
        let grant = make_grant(
            "ns-target",
            "g1",
            "ns-route",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            Some("allowed-svc"),
        );
        let store = store_with(vec![("ns-target/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "other-svc", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_route_backend_allowed_when_to_name_matches_explicit() {
        // Grant restricts to "allowed-svc"; ref targets exactly that name.
        let grant = make_grant(
            "ns-target",
            "g1",
            "ns-route",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            Some("allowed-svc"),
        );
        let store = store_with(vec![("ns-target/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "allowed-svc", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert!(
            errors.is_empty(),
            "explicit name match should be allowed, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_route_backend_kind_default_is_service_when_none() {
        // backend.kind = None must be normalized to "Service".
        // Grant only allows Service, ref omits kind -> should still match.
        let grant = make_grant(
            "ns-target",
            "g1",
            "ns-route",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            None,
        );
        let store = store_with(vec![("ns-target/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", None, "svc-a", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert!(
            errors.is_empty(),
            "kind=None should default to Service, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_route_backend_multiple_grants_one_match_is_enough() {
        // Two grants in the same target namespace; the second one matches.
        let grant_no_match = make_grant(
            "ns-target",
            "g1",
            "ns-other",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            None,
        );
        let grant_match = make_grant(
            "ns-target",
            "g2",
            "ns-route",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            None,
        );
        let store = store_with(vec![
            ("ns-target/g1".to_string(), grant_no_match),
            ("ns-target/g2".to_string(), grant_match),
        ]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert!(errors.is_empty());
    }

    #[test]
    fn test_route_backend_mixed_refs_only_disallowed_reported() {
        // First ref: same ns (allowed). Second: cross-ns with grant (allowed).
        // Third: cross-ns to namespace with no grant (rejected).
        let grant = make_grant(
            "ns-target",
            "g1",
            "ns-route",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            None,
        );
        let store = store_with(vec![("ns-target/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![
            backend_ref("", Some("Service"), "svc-local", Some("ns-route")),
            backend_ref("", Some("Service"), "svc-allowed", Some("ns-target")),
            backend_ref("", Some("Service"), "svc-blocked", Some("ns-forbidden")),
        ];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert_eq!(errors.len(), 1, "only the third ref should fail, got: {:?}", errors);
        assert!(errors[0].contains("svc-blocked"));
        assert!(errors[0].contains("ns-forbidden"));
    }

    #[test]
    fn test_route_backend_grant_in_wrong_namespace_does_not_help() {
        // Grant lives in ns-other (not ns-target). Ref targets ns-target -> rejected.
        let grant = make_grant(
            "ns-other",
            "g1",
            "ns-route",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            None,
        );
        let store = store_with(vec![("ns-other/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert_eq!(errors.len(), 1);
    }

    // ---- Gateway certificate ref tests --------------------------------------

    #[test]
    fn test_gateway_cert_same_namespace_is_always_allowed() {
        let store = store_with(vec![]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![cert_ref(Some(""), Some("Secret"), "tls-cert", Some("ns-gw"))];
        let errors = validator.validate_gateway_certificate_refs("ns-gw", &refs);

        assert!(errors.is_empty());
    }

    #[test]
    fn test_gateway_cert_cross_ns_allowed_by_grant() {
        let grant = make_grant(
            "ns-certs",
            "g1",
            "ns-gw",
            "gateway.networking.k8s.io",
            "Gateway",
            "",
            "Secret",
            None,
        );
        let store = store_with(vec![("ns-certs/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![cert_ref(Some(""), Some("Secret"), "tls-cert", Some("ns-certs"))];
        let errors = validator.validate_gateway_certificate_refs("ns-gw", &refs);

        assert!(
            errors.is_empty(),
            "cross-ns cert ref should be allowed, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_gateway_cert_cross_ns_rejected_without_grant() {
        let store = store_with(vec![]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![cert_ref(Some(""), Some("Secret"), "tls-cert", Some("ns-certs"))];
        let errors = validator.validate_gateway_certificate_refs("ns-gw", &refs);

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Gateway"));
        assert!(errors[0].contains("Secret"));
        assert!(errors[0].contains("tls-cert"));
        assert!(errors[0].contains("ns-certs"));
        assert!(errors[0].contains("ns-gw"));
    }

    #[test]
    fn test_gateway_cert_rejected_when_to_name_mismatch() {
        // Grant restricts to "allowed-cert", ref is for "other-cert".
        let grant = make_grant(
            "ns-certs",
            "g1",
            "ns-gw",
            "gateway.networking.k8s.io",
            "Gateway",
            "",
            "Secret",
            Some("allowed-cert"),
        );
        let store = store_with(vec![("ns-certs/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![cert_ref(Some(""), Some("Secret"), "other-cert", Some("ns-certs"))];
        let errors = validator.validate_gateway_certificate_refs("ns-gw", &refs);

        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_gateway_cert_group_and_kind_default_to_core_secret() {
        // SecretObjectReference: group=None and kind=None must default to ("", "Secret").
        let grant = make_grant(
            "ns-certs",
            "g1",
            "ns-gw",
            "gateway.networking.k8s.io",
            "Gateway",
            "",
            "Secret",
            None,
        );
        let store = store_with(vec![("ns-certs/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![cert_ref(None, None, "tls-cert", Some("ns-certs"))];
        let errors = validator.validate_gateway_certificate_refs("ns-gw", &refs);

        assert!(
            errors.is_empty(),
            "default group/kind should resolve to core/Secret, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_route_backend_grant_for_gateway_does_not_authorize_route() {
        // Grant only trusts Gateway as the from-kind. An HTTPRoute must not benefit.
        let grant = make_grant(
            "ns-target",
            "g1",
            "ns-route",
            "gateway.networking.k8s.io",
            "Gateway",
            "",
            "Service",
            None,
        );
        let store = store_with(vec![("ns-target/g1".to_string(), grant)]);
        let validator = CrossNamespaceValidator::from_store(store);

        let refs = vec![backend_ref("", Some("Service"), "svc-a", Some("ns-target"))];
        let errors = validator.validate_route_backend_refs("ns-route", "HTTPRoute", &refs);

        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_normalize_helpers_defaults() {
        // Direct coverage of the small helpers used by is_cross_ns_ref_allowed.
        assert_eq!(normalize_group(None), "");
        assert_eq!(normalize_group(Some(&"".to_string())), "");
        assert_eq!(normalize_group(Some(&"my.group".to_string())), "my.group");

        assert_eq!(normalize_kind(None), "Service");
        assert_eq!(normalize_kind(Some(&"Secret".to_string())), "Secret");
    }
}
