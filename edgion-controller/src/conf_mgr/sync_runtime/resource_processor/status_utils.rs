//! Status utilities for Gateway API resources
//!
//! Provides utilities for managing resource status according to Gateway API standards (GEP-1364):
//! - Standard conditions: Accepted, ResolvedRefs
//! - Typed error enums for compile-time safe reason mapping
//! - Condition update helpers
//! - Time formatting
//!
//! Note: Programmed is set on Gateway listeners based on control-plane validation
//! (Accepted=True and no conflicts). Ready conditions are not implemented as they
//! require data-plane feedback unavailable in our async gRPC architecture.
//! PartiallyInvalid condition is supported for routes with partially invalid rules.

use chrono::{SecondsFormat, Utc};
use edgion_resources::resources::common::Condition;

/// Standard Gateway API condition types
pub mod condition_types {
    /// Resource is syntactically and semantically valid
    pub const ACCEPTED: &str = "Accepted";
    /// All references to other objects are resolved
    pub const RESOLVED_REFS: &str = "ResolvedRefs";
    /// Listener conflicts with another Listener (port/hostname collision)
    pub const CONFLICTED: &str = "Conflicted";
    /// Gateway has listeners that are not valid (used at Gateway level)
    pub const LISTENERS_NOT_VALID: &str = "ListenersNotValid";
    /// Some rules in the route are invalid but others are valid
    pub const PARTIALLY_INVALID: &str = "PartiallyInvalid";
    /// Listener configuration has been sent to the data plane (listener-level)
    pub const PROGRAMMED: &str = "Programmed";
}

/// Standard Gateway API condition reasons
pub mod condition_reasons {
    // Accepted reasons
    pub const ACCEPTED: &str = "Accepted";
    pub const INVALID_ROUTE_KIND: &str = "InvalidRouteKinds";
    pub const NO_MATCHING_PARENT: &str = "NoMatchingParent";
    pub const NOT_ALLOWED_BY_LISTENERS: &str = "NotAllowedByListeners";
    /// Route hostnames don't intersect with the listener's hostname (Gateway API v1.1+)
    pub const NO_MATCHING_LISTENER_HOSTNAME: &str = "NoMatchingListenerHostname";
    /// GatewayClass parametersRef target is invalid (bad group/kind) or not found.
    pub const INVALID_PARAMETERS: &str = "InvalidParameters";

    // ResolvedRefs reasons
    pub const RESOLVED_REFS: &str = "ResolvedRefs";
    pub const REF_NOT_PERMITTED: &str = "RefNotPermitted";
    pub const BACKEND_NOT_FOUND: &str = "BackendNotFound";
    pub const INVALID_KIND: &str = "InvalidKind";

    // ResolvedRefs reasons for CRDs that resolve Kubernetes Secrets for
    // credentials (e.g. `LinkSys`, `EdgionAcme`, `EdgionPlugins`, `EdgionTls`).
    // These are distinct from the Gateway-API backend-ref reasons above; they
    // pinpoint *why* the referenced Secret could not be turned into usable
    // credential material.
    //
    // NOTE: These four reasons are **Edgion extensions**, not Gateway API
    // upstream reasons. Third-party Gateway-API tooling (Envoy Gateway / Kuma
    // dashboards, etc.) will not recognise them by name. Cross-namespace
    // denial intentionally uses the upstream `RefNotPermitted` and is *not*
    // split further — the pipeline cannot distinguish "no grant yet" from
    // "grant denies", so a more specific reason would be misleading.
    /// Edgion extension · The referenced Secret does not exist (or has not yet been cached).
    pub const SECRET_NOT_FOUND: &str = "SecretNotFound";
    // NOTE: There is intentionally *no* `REFERENCE_GRANT_MISSING` /
    // `CROSS_NS_REF_NOT_PERMITTED` / similar reason here. Cross-namespace
    // denial uses the upstream `REF_NOT_PERMITTED`: the pipeline
    // cannot distinguish "no grant yet" from "grant denies", so advertising a
    // distinction via a dedicated reason would mislead operators.
    /// Edgion extension · The Secret exists but is missing one or more required `data` keys.
    pub const MISSING_KEYS: &str = "MissingKeys";
    /// Edgion extension · A Secret data value could not be decoded (non-UTF-8, bad JSON, ...).
    pub const DECODE_ERROR: &str = "DecodeError";
    /// Edgion extension · The `Secret.type` is not in the resolver's accepted whitelist.
    pub const INVALID_SECRET_TYPE: &str = "InvalidSecretType";

    // General reasons
    pub const INVALID: &str = "Invalid";
    /// TLS listener mode is not supported (e.g. Mixed mode for TLSRoute)
    pub const UNSUPPORTED_TLS_MODE: &str = "UnsupportedTLSMode";
    /// Listener field value is not supported by the current implementation.
    pub const UNSUPPORTED_VALUE: &str = "UnsupportedValue";
    /// HTTPS/TLS listener missing tls block or certificateRefs
    pub const INVALID_TLS_CONFIGURATION: &str = "InvalidTLSConfiguration";

    // Conflicted reasons (for Listener port conflicts)
    pub const LISTENER_CONFLICT: &str = "ListenerConflict";
    pub const PROTOCOL_CONFLICT: &str = "ProtocolConflict";
    pub const NO_CONFLICTS: &str = "NoConflicts";

    // Policy conflict reason (for policy attachment conflicts)
    /// Another policy targeting the same resource has higher precedence
    pub const POLICY_CONFLICTED: &str = "Conflicted";

    // BackendTLSPolicy-specific reasons (Gateway API v1.5 conformance)
    /// ALL CACertificateRefs are invalid — Accepted condition must be False.
    pub const NO_VALID_CA_CERTIFICATE: &str = "NoValidCACertificate";
    /// CACertificateRef cannot be resolved (resource not found or misconfigured).
    pub const INVALID_CA_CERTIFICATE_REF: &str = "InvalidCACertificateRef";

    /// Edgion extension · Referenced EdgionConfigData not found or has an
    /// unexpected variant type (e.g. `ServiceRegionRoute.clusterRef` pointing
    /// at a non-existent or non-`ClusterRegionRoute` EdgionConfigData). Not a
    /// Gateway API upstream reason — see the "Edgion extension" note above
    /// the Secret-focused reasons for the general caveat.
    pub const INVALID_EDGION_CONFIG_DATA_REF: &str = "InvalidEdgionConfigDataRef";
    /// Gateway-level: edgion.io/edgion-stream-plugins annotation references a missing resource.
    pub const INVALID_STREAM_PLUGINS_REF: &str = "InvalidStreamPluginsRef";

    // Listener ResolvedRefs reasons (Gateway API v1.5)
    /// Listener certificate Secret ref invalid or unresolvable.
    pub const INVALID_CERTIFICATE_REF: &str = "InvalidCertificateRef";
    /// Edgion extension · The referenced Secret's certificate content is unusable
    /// (expired / SAN mismatch / key mismatch / unparseable). Distinct from
    /// `INVALID_CERTIFICATE_REF`, which signals a bad *reference*; this signals
    /// bad *content* in an otherwise-resolved reference.
    pub const INVALID_CERTIFICATE: &str = "InvalidCertificate";
    /// Listener CA certificate ref kind unsupported.
    pub const INVALID_CA_CERTIFICATE_KIND: &str = "InvalidCACertificateKind";

    // Programmed reasons (for listener Programmed condition)
    pub const PROGRAMMED: &str = "Programmed";
    pub const INVALID_LISTENER: &str = "Invalid";

    // PartiallyInvalid reasons
    pub const RULE_INVALID: &str = "RuleInvalid";
    pub const ALL_RULES_VALID: &str = "AllRulesValid";
}

// ============================================================================
// Typed error enums — replace string-based reason inference
// ============================================================================

/// Typed errors for Accepted condition.
///
/// Each variant maps to a specific Gateway API reason string, eliminating
/// string-based inference and providing compile-time safety.
#[derive(Debug, Clone, PartialEq)]
pub enum AcceptedError {
    /// Resource namespace not allowed by listener's allowedRoutes policy
    NotAllowedByListeners { resource_ns: String },
    /// Route kind not allowed by listener's allowedRoutes.kinds policy
    NotAllowedByListenerKind { resource_kind: String },
    /// Route hostnames don't intersect with any listener hostname
    NoMatchingListenerHostname { hostnames: Vec<String> },
    /// sectionName doesn't match any listener
    NoMatchingParent { section_name: String },
    /// Referenced parent Gateway not yet present in the store (init window)
    ParentNotFound { parent_name: String },
    /// Resource failed validation or preparse (generic validation error)
    Invalid { message: String },
}

impl AcceptedError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::NotAllowedByListeners { .. } => condition_reasons::NOT_ALLOWED_BY_LISTENERS,
            Self::NotAllowedByListenerKind { .. } => condition_reasons::NOT_ALLOWED_BY_LISTENERS,
            Self::NoMatchingListenerHostname { .. } => condition_reasons::NO_MATCHING_LISTENER_HOSTNAME,
            Self::NoMatchingParent { .. } => condition_reasons::NO_MATCHING_PARENT,
            Self::ParentNotFound { .. } => condition_reasons::NO_MATCHING_PARENT,
            Self::Invalid { .. } => condition_reasons::INVALID,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NotAllowedByListeners { resource_ns } => {
                format!("Namespace '{}' not allowed by Gateway listeners", resource_ns)
            }
            Self::NotAllowedByListenerKind { resource_kind } => {
                format!("Route kind '{}' not allowed by Gateway listeners", resource_kind)
            }
            Self::NoMatchingListenerHostname { hostnames } => {
                format!("No matching hostname for route hostnames {:?}", hostnames)
            }
            Self::NoMatchingParent { section_name } => {
                format!("No matching listener for sectionName '{}'", section_name)
            }
            Self::ParentNotFound { parent_name } => {
                format!("Parent Gateway '{}' not found", parent_name)
            }
            Self::Invalid { message } => message.clone(),
        }
    }

    pub fn from_validation_errors(errors: &[String]) -> Vec<Self> {
        errors.iter().map(|e| Self::Invalid { message: e.clone() }).collect()
    }
}

/// Typed errors for ResolvedRefs condition.
///
/// Each variant maps to a specific Gateway API reason string, eliminating
/// string-based inference and providing compile-time safety.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedRefsError {
    /// Backend kind not supported (e.g., not "Service")
    InvalidKind { kind: String, name: String },
    /// Backend Service not found
    BackendNotFound { namespace: String, name: String },
    /// Referenced Secret not found
    SecretNotFound { namespace: String, name: String },
    /// Cross-namespace reference denied by ReferenceGrant policy.
    ///
    /// Carries the referrer's `(from_namespace, from_kind)` so the message
    /// stays self-describing when scraped out of events / aggregated log
    /// pipelines where the owning CRD's `metadata.namespace` is no longer in
    /// context. `from_kind` is `&'static str` because every call site knows
    /// its `ResourceKind` at compile time (via `ResourceKind::as_str()` or a
    /// string literal).
    RefNotPermitted {
        from_namespace: String,
        from_kind: &'static str,
        target_namespace: String,
        target_name: String,
    },
    /// Referenced EdgionConfigData not found, or found but its data variant
    /// type doesn't match what the referrer requires (e.g. ServiceRegionRoute.clusterRef
    /// expects an EdgionConfigData of type ClusterRegionRoute).
    EdgionConfigDataNotFound {
        namespace: String,
        name: String,
        expected_type: String,
    },
    /// Edgion extension · The Secret exists but is missing one or more required
    /// `data` keys (e.g. `password` for basic-auth, `tls.key` for TLS certs).
    MissingKeys { keys: Vec<&'static str> },
    /// Edgion extension · A Secret data value could not be decoded (non-UTF-8,
    /// malformed JSON, etc.). `key` identifies the offending field.
    DecodeError { key: &'static str, detail: String },
    /// Edgion extension · The `Secret.type` is not in the resolver's accepted
    /// whitelist (e.g. pointing an `kubernetes.io/tls` Secret at a basic-auth
    /// credential slot).
    InvalidSecretType {
        got: String,
        allowed: &'static [&'static str],
    },
    /// Edgion extension · The referenced Secret's certificate content is unusable
    /// (expired / SAN mismatch / key mismatch / unparseable). `detail` comes from
    /// `CertValidationError::status_message()` and MUST be deterministic (no
    /// wall-clock) — a varying status message would cause an infinite status-write
    /// loop (see the tls-06 design). The condition reason is the coarse
    /// `INVALID_CERTIFICATE`; `detail` carries the specific human-readable cause.
    CertContentInvalid { detail: String },
}

impl ResolvedRefsError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::InvalidKind { .. } => condition_reasons::INVALID_KIND,
            Self::BackendNotFound { .. } => condition_reasons::BACKEND_NOT_FOUND,
            Self::SecretNotFound { .. } => condition_reasons::SECRET_NOT_FOUND,
            Self::RefNotPermitted { .. } => condition_reasons::REF_NOT_PERMITTED,
            Self::EdgionConfigDataNotFound { .. } => condition_reasons::INVALID_EDGION_CONFIG_DATA_REF,
            Self::MissingKeys { .. } => condition_reasons::MISSING_KEYS,
            Self::DecodeError { .. } => condition_reasons::DECODE_ERROR,
            Self::InvalidSecretType { .. } => condition_reasons::INVALID_SECRET_TYPE,
            Self::CertContentInvalid { .. } => condition_reasons::INVALID_CERTIFICATE,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::InvalidKind { kind, name } => {
                format!("Invalid backend ref kind '{}' for backend '{}'", kind, name)
            }
            Self::BackendNotFound { namespace, name } => {
                format!("Service '{}/{}' not found", namespace, name)
            }
            Self::SecretNotFound { namespace, name } => {
                format!("Secret '{}/{}' not found", namespace, name)
            }
            Self::RefNotPermitted {
                from_namespace,
                from_kind,
                target_namespace,
                target_name,
            } => {
                format!(
                    "Reference from {}/{} to \"{}/{}\" is not permitted without a ReferenceGrant",
                    from_namespace, from_kind, target_namespace, target_name
                )
            }
            Self::EdgionConfigDataNotFound {
                namespace,
                name,
                expected_type,
            } => {
                format!(
                    "EdgionConfigData '{}/{}' (expected type={}) not found",
                    namespace, name, expected_type
                )
            }
            Self::MissingKeys { keys } => {
                format!("Secret is missing required data keys: {}", keys.join(", "))
            }
            Self::DecodeError { key, detail } => {
                format!("credential decode failed for key '{}': {}", key, detail)
            }
            Self::InvalidSecretType { got, allowed } => {
                format!(
                    "Secret.type {:?} is not accepted for this credential (allowed: {:?})",
                    got, allowed
                )
            }
            Self::CertContentInvalid { detail } => detail.clone(),
        }
    }
}

// ============================================================================
// Condition helpers
// ============================================================================

/// Get current time in RFC3339 format
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Create a condition with True status
pub fn condition_true(
    type_: &str,
    reason: &str,
    message: impl Into<String>,
    observed_generation: Option<i64>,
) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "True".to_string(),
        reason: reason.to_string(),
        message: message.into(),
        last_transition_time: now_rfc3339(),
        observed_generation,
    }
}

/// Create a condition with False status
pub fn condition_false(
    type_: &str,
    reason: &str,
    message: impl Into<String>,
    observed_generation: Option<i64>,
) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "False".to_string(),
        reason: reason.to_string(),
        message: message.into(),
        last_transition_time: now_rfc3339(),
        observed_generation,
    }
}

/// Create a condition with Unknown status
pub fn condition_unknown(
    type_: &str,
    reason: &str,
    message: impl Into<String>,
    observed_generation: Option<i64>,
) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "Unknown".to_string(),
        reason: reason.to_string(),
        message: message.into(),
        last_transition_time: now_rfc3339(),
        observed_generation,
    }
}

/// Update or insert a condition in a conditions list
///
/// If a condition with the same type exists:
/// - Only update last_transition_time if status actually changed
/// - Always update reason, message, and observed_generation
///
/// If no condition with the type exists:
/// - Insert the new condition
pub fn update_condition(conditions: &mut Vec<Condition>, new_condition: Condition) {
    if let Some(existing) = conditions.iter_mut().find(|c| c.type_ == new_condition.type_) {
        // Only update last_transition_time if status changed
        let status_changed = existing.status != new_condition.status;

        existing.status = new_condition.status;
        existing.reason = new_condition.reason;
        existing.message = new_condition.message;
        existing.observed_generation = new_condition.observed_generation;

        if status_changed {
            existing.last_transition_time = new_condition.last_transition_time;
        }
    } else {
        conditions.push(new_condition);
    }
}

/// Create standard "Accepted: True" condition
pub fn accepted_condition(observed_generation: Option<i64>) -> Condition {
    accepted_condition_with_message(observed_generation, "Resource accepted")
}

/// Create standard "Accepted: True" condition with custom message
pub fn accepted_condition_with_message(observed_generation: Option<i64>, message: impl Into<String>) -> Condition {
    condition_true(
        condition_types::ACCEPTED,
        condition_reasons::ACCEPTED,
        message,
        observed_generation,
    )
}

/// Create standard "ResolvedRefs" condition based on typed validation errors
pub fn resolved_refs_condition(errors: &[ResolvedRefsError], observed_generation: Option<i64>) -> Condition {
    if errors.is_empty() {
        condition_true(
            condition_types::RESOLVED_REFS,
            condition_reasons::RESOLVED_REFS,
            "All references resolved",
            observed_generation,
        )
    } else {
        let reason = pick_resolved_refs_reason(errors);
        let message = errors.iter().map(|e| e.message()).collect::<Vec<_>>().join("; ");
        condition_false(condition_types::RESOLVED_REFS, reason, message, observed_generation)
    }
}

/// Pick the highest-priority reason from a set of ResolvedRefs errors.
///
/// Priority order (matching Gateway API spec precedence, with Edgion Secret
/// extensions slotted by severity; lower number = higher priority):
/// 1. InvalidKind — fundamentally wrong backend/ref type
/// 2. RefNotPermitted — policy denial
/// 3. CertContentInvalid — certificate content unusable (expired / SAN / key mismatch)
/// 4. InvalidSecretType — Secret exists but wrong type (akin to InvalidKind, but for Secrets)
/// 5. EdgionConfigDataNotFound — dependent EdgionConfigData missing
/// 6. BackendNotFound — target Service missing
/// 7. SecretNotFound — target Secret missing
/// 8. DecodeError — Secret content unusable (parse failure)
/// 9. MissingKeys — Secret content incomplete
///
/// Exposed as `pub(crate)` so other handlers (e.g. `EdgionPluginsHandler`,
/// which still maintains its own Condition construction) can use
/// the same deterministic priority instead of `errors[0].reason()`.
///
/// **Empty slice** returns a sentinel — callers guarantee non-empty at the
/// `resolved_refs_condition` level. See also the `variant_priority` helper:
/// adding a new `ResolvedRefsError` variant is enforced at compile time to
/// declare its priority (the `match` is exhaustive).
pub(crate) fn pick_resolved_refs_reason(errors: &[ResolvedRefsError]) -> &'static str {
    // Exhaustive match on every variant forces future contributors to assign
    // a priority slot for any new variant — the compiler refuses to compile
    // otherwise, preventing silent "fall through to generic fallback" regressions.
    fn variant_priority(err: &ResolvedRefsError) -> u8 {
        match err {
            ResolvedRefsError::InvalidKind { .. } => 0,
            ResolvedRefsError::RefNotPermitted { .. } => 1,
            ResolvedRefsError::CertContentInvalid { .. } => 2,
            ResolvedRefsError::InvalidSecretType { .. } => 3,
            ResolvedRefsError::EdgionConfigDataNotFound { .. } => 4,
            ResolvedRefsError::BackendNotFound { .. } => 5,
            ResolvedRefsError::SecretNotFound { .. } => 6,
            ResolvedRefsError::DecodeError { .. } => 7,
            ResolvedRefsError::MissingKeys { .. } => 8,
        }
    }

    match errors.iter().min_by_key(|e| variant_priority(e)) {
        Some(e) => e.reason(),
        // Unreachable in practice — `resolved_refs_condition` short-circuits
        // to `ResolvedRefs=True/RESOLVED_REFS` before calling this helper.
        // If we ever do reach here with an empty slice, `Invalid` is a safe
        // default for a `status=False` condition (generic "something's wrong"
        // reason) — never returning `ResolvedRefs` because that constant is
        // the *success* reason and `status=False + reason=ResolvedRefs` is
        // internally contradictory under Gateway API semantics.
        None => condition_reasons::INVALID,
    }
}

/// Set standard conditions (Accepted, ResolvedRefs) for a resource's parent status.
/// Uses empty `accepted_errors`; `resolved_refs_errors` controls the ResolvedRefs condition.
pub fn set_parent_conditions(
    conditions: &mut Vec<Condition>,
    resolved_refs_errors: &[ResolvedRefsError],
    observed_generation: Option<i64>,
) {
    set_parent_conditions_full(conditions, &[], resolved_refs_errors, observed_generation);
}

/// Set standard conditions (Accepted, ResolvedRefs) for a resource's parent status.
/// `accepted_errors` controls the Accepted condition per-parent.
/// `resolved_refs_errors` controls the ResolvedRefs condition (resource-level).
pub fn set_parent_conditions_full(
    conditions: &mut Vec<Condition>,
    accepted_errors: &[AcceptedError],
    resolved_refs_errors: &[ResolvedRefsError],
    observed_generation: Option<i64>,
) {
    if accepted_errors.is_empty() {
        update_condition(conditions, accepted_condition(observed_generation));
    } else {
        let reason = accepted_errors[0].reason();
        let message = accepted_errors
            .iter()
            .map(|e| e.message())
            .collect::<Vec<_>>()
            .join("; ");
        update_condition(
            conditions,
            condition_false(condition_types::ACCEPTED, reason, message, observed_generation),
        );
    }
    update_condition(
        conditions,
        resolved_refs_condition(resolved_refs_errors, observed_generation),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_condition_insert() {
        let mut conditions = Vec::new();
        let new_cond = condition_true("TestType", "TestReason", "Test message", Some(1));

        update_condition(&mut conditions, new_cond);

        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].type_, "TestType");
        assert_eq!(conditions[0].status, "True");
    }

    #[test]
    fn test_update_condition_status_change() {
        let mut conditions = vec![condition_true("TestType", "OldReason", "Old message", Some(1))];
        conditions[0].last_transition_time = "1970-01-01T00:00:00Z".to_string();

        let old_time = conditions[0].last_transition_time.clone();

        // Update with same status - time should NOT change
        let same_status = condition_true("TestType", "NewReason", "New message", Some(2));
        update_condition(&mut conditions, same_status);
        assert_eq!(conditions[0].last_transition_time, old_time);
        assert_eq!(conditions[0].reason, "NewReason");

        // Update with different status - time SHOULD change
        let diff_status = condition_false("TestType", "FailReason", "Failed", Some(3));
        update_condition(&mut conditions, diff_status);
        assert_ne!(conditions[0].last_transition_time, old_time);
        assert_eq!(conditions[0].status, "False");
    }

    #[test]
    fn test_resolved_refs_condition_no_errors() {
        let cond = resolved_refs_condition(&[], Some(1));
        assert_eq!(cond.status, "True");
        assert_eq!(cond.reason, "ResolvedRefs");
    }

    #[test]
    fn test_accepted_error_not_allowed_by_listeners() {
        let err = AcceptedError::NotAllowedByListeners {
            resource_ns: "test-ns".to_string(),
        };
        assert_eq!(err.reason(), "NotAllowedByListeners");
        assert_eq!(err.message(), "Namespace 'test-ns' not allowed by Gateway listeners");
    }

    #[test]
    fn test_accepted_error_not_allowed_by_listener_kind() {
        let err = AcceptedError::NotAllowedByListenerKind {
            resource_kind: "GRPCRoute".to_string(),
        };
        assert_eq!(err.reason(), "NotAllowedByListeners");
        assert_eq!(err.message(), "Route kind 'GRPCRoute' not allowed by Gateway listeners");
    }

    #[test]
    fn test_accepted_error_no_matching_listener_hostname() {
        let err = AcceptedError::NoMatchingListenerHostname {
            hostnames: vec!["a.example.com".to_string(), "b.example.com".to_string()],
        };
        assert_eq!(err.reason(), "NoMatchingListenerHostname");
        assert!(err.message().contains("a.example.com"));
        assert!(err.message().contains("b.example.com"));
    }

    #[test]
    fn test_accepted_error_no_matching_parent() {
        let err = AcceptedError::NoMatchingParent {
            section_name: "https".to_string(),
        };
        assert_eq!(err.reason(), "NoMatchingParent");
        assert_eq!(err.message(), "No matching listener for sectionName 'https'");
    }

    #[test]
    fn test_accepted_error_parent_not_found() {
        let err = AcceptedError::ParentNotFound {
            parent_name: "default/my-gateway".to_string(),
        };
        assert_eq!(err.reason(), "NoMatchingParent");
        assert_eq!(err.message(), "Parent Gateway 'default/my-gateway' not found");
    }

    #[test]
    fn test_resolved_refs_error_invalid_kind() {
        let err = ResolvedRefsError::InvalidKind {
            kind: "Deployment".to_string(),
            name: "my-deploy".to_string(),
        };
        assert_eq!(err.reason(), "InvalidKind");
        assert_eq!(
            err.message(),
            "Invalid backend ref kind 'Deployment' for backend 'my-deploy'"
        );
    }

    #[test]
    fn test_resolved_refs_error_backend_not_found() {
        let err = ResolvedRefsError::BackendNotFound {
            namespace: "default".to_string(),
            name: "my-svc".to_string(),
        };
        assert_eq!(err.reason(), "BackendNotFound");
        assert_eq!(err.message(), "Service 'default/my-svc' not found");
    }

    #[test]
    fn test_resolved_refs_error_ref_not_permitted() {
        let err = ResolvedRefsError::RefNotPermitted {
            from_namespace: "app-ns".to_string(),
            from_kind: "HTTPRoute",
            target_namespace: "other-ns".to_string(),
            target_name: "other-svc".to_string(),
        };
        assert_eq!(err.reason(), "RefNotPermitted");
        assert_eq!(
            err.message(),
            "Reference from app-ns/HTTPRoute to \"other-ns/other-svc\" is not permitted without a ReferenceGrant"
        );
    }

    #[test]
    fn test_resolved_refs_condition_single_error() {
        let errors = vec![ResolvedRefsError::BackendNotFound {
            namespace: "default".to_string(),
            name: "svc-a".to_string(),
        }];
        let cond = resolved_refs_condition(&errors, Some(1));
        assert_eq!(cond.status, "False");
        assert_eq!(cond.reason, "BackendNotFound");
        assert!(cond.message.contains("svc-a"));
    }

    #[test]
    fn test_resolved_refs_reason_priority_invalid_kind_highest() {
        let errors = vec![
            ResolvedRefsError::RefNotPermitted {
                from_namespace: "app-ns".to_string(),
                from_kind: "HTTPRoute",
                target_namespace: "ns".to_string(),
                target_name: "svc".to_string(),
            },
            ResolvedRefsError::InvalidKind {
                kind: "Deployment".to_string(),
                name: "deploy".to_string(),
            },
            ResolvedRefsError::BackendNotFound {
                namespace: "default".to_string(),
                name: "svc2".to_string(),
            },
        ];
        let cond = resolved_refs_condition(&errors, Some(1));
        assert_eq!(cond.reason, "InvalidKind");
    }

    #[test]
    fn test_resolved_refs_reason_priority_ref_not_permitted_over_backend() {
        let errors = vec![
            ResolvedRefsError::BackendNotFound {
                namespace: "default".to_string(),
                name: "svc".to_string(),
            },
            ResolvedRefsError::RefNotPermitted {
                from_namespace: "app-ns".to_string(),
                from_kind: "HTTPRoute",
                target_namespace: "other".to_string(),
                target_name: "svc2".to_string(),
            },
        ];
        let cond = resolved_refs_condition(&errors, Some(1));
        assert_eq!(cond.reason, "RefNotPermitted");
    }

    #[test]
    fn test_resolved_refs_condition_message_joins_multiple() {
        let errors = vec![
            ResolvedRefsError::BackendNotFound {
                namespace: "a".to_string(),
                name: "svc1".to_string(),
            },
            ResolvedRefsError::BackendNotFound {
                namespace: "b".to_string(),
                name: "svc2".to_string(),
            },
        ];
        let cond = resolved_refs_condition(&errors, Some(1));
        assert!(cond.message.contains("a/svc1"));
        assert!(cond.message.contains("b/svc2"));
        assert!(cond.message.contains("; "));
    }

    #[test]
    fn test_set_parent_conditions_no_errors() {
        let mut conditions = Vec::new();
        set_parent_conditions(&mut conditions, &[], Some(1));

        assert_eq!(conditions.len(), 2); // Accepted, ResolvedRefs
        assert!(conditions.iter().all(|c| c.status == "True"));
        assert!(conditions.iter().any(|c| c.type_ == "Accepted"));
        assert!(conditions.iter().any(|c| c.type_ == "ResolvedRefs"));
        assert!(!conditions.iter().any(|c| c.type_ == "Programmed"));
        assert!(!conditions.iter().any(|c| c.type_ == "Ready"));
    }

    #[test]
    fn test_set_parent_conditions_with_resolved_refs_errors() {
        let mut conditions = Vec::new();
        let errors = vec![ResolvedRefsError::RefNotPermitted {
            from_namespace: "app-ns".to_string(),
            from_kind: "HTTPRoute",
            target_namespace: "ns".to_string(),
            target_name: "svc".to_string(),
        }];
        set_parent_conditions(&mut conditions, &errors, Some(1));

        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "True");

        let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, "RefNotPermitted");
    }

    #[test]
    fn test_set_parent_conditions_full_with_accepted_errors() {
        let mut conditions = Vec::new();
        let accepted_errors = vec![AcceptedError::NotAllowedByListeners {
            resource_ns: "blocked-ns".to_string(),
        }];
        set_parent_conditions_full(&mut conditions, &accepted_errors, &[], Some(1));

        assert_eq!(conditions.len(), 2); // Accepted, ResolvedRefs only

        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, "NotAllowedByListeners");
        assert!(accepted.message.contains("blocked-ns"));

        assert!(!conditions.iter().any(|c| c.type_ == "Programmed"));
        assert!(!conditions.iter().any(|c| c.type_ == "Ready"));

        let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        assert_eq!(resolved.status, "True");
    }

    #[test]
    fn test_set_parent_conditions_full_with_both_errors() {
        let mut conditions = Vec::new();
        let accepted_errors = vec![AcceptedError::NoMatchingParent {
            section_name: "nonexistent".to_string(),
        }];
        let resolved_errors = vec![ResolvedRefsError::BackendNotFound {
            namespace: "default".to_string(),
            name: "missing-svc".to_string(),
        }];
        set_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_errors, Some(1));

        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, "NoMatchingParent");

        let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, "BackendNotFound");
    }

    #[test]
    fn test_accepted_error_invalid() {
        let err = AcceptedError::Invalid {
            message: "Secret 'ns/secret' not found".to_string(),
        };
        assert_eq!(err.reason(), "Invalid");
        assert_eq!(err.message(), "Secret 'ns/secret' not found");
    }

    #[test]
    fn test_accepted_error_from_validation_errors_empty() {
        let errors = AcceptedError::from_validation_errors(&[]);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_accepted_error_from_validation_errors_multiple() {
        let validation = vec![
            "Secret 'ns/secret' not found".to_string(),
            "No parent_refs configured".to_string(),
        ];
        let errors = AcceptedError::from_validation_errors(&validation);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].reason(), "Invalid");
        assert_eq!(errors[0].message(), "Secret 'ns/secret' not found");
        assert_eq!(errors[1].message(), "No parent_refs configured");
    }

    #[test]
    fn test_set_parent_conditions_full_with_validation_errors() {
        let mut conditions = Vec::new();
        let accepted_errors =
            AcceptedError::from_validation_errors(&["Secret 'test/missing' not found (may arrive later)".to_string()]);
        set_parent_conditions_full(&mut conditions, &accepted_errors, &[], Some(1));

        assert_eq!(conditions.len(), 2); // Accepted, ResolvedRefs only

        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, "Invalid");
        assert!(accepted.message.contains("Secret 'test/missing' not found"));

        assert!(!conditions.iter().any(|c| c.type_ == "Programmed"));
        assert!(!conditions.iter().any(|c| c.type_ == "Ready"));

        let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        assert_eq!(resolved.status, "True");
    }

    #[test]
    fn test_resolved_refs_error_secret_not_found_reason_is_secret_not_found() {
        // SecretNotFound must map to SECRET_NOT_FOUND, not BACKEND_NOT_FOUND —
        // using BackendNotFound would mislead operators of Secret-backed CRDs
        // (LinkSys / EdgionPlugins / EdgionTls) into looking for a missing Service.
        let err = ResolvedRefsError::SecretNotFound {
            namespace: "ns".to_string(),
            name: "missing".to_string(),
        };
        assert_eq!(err.reason(), "SecretNotFound");
        assert!(err.message().contains("ns/missing"));
    }

    #[test]
    fn test_resolved_refs_error_missing_keys() {
        let err = ResolvedRefsError::MissingKeys {
            keys: vec!["password", "username"],
        };
        assert_eq!(err.reason(), "MissingKeys");
        assert!(err.message().contains("password"));
        assert!(err.message().contains("username"));
    }

    #[test]
    fn test_resolved_refs_error_decode_error() {
        let err = ResolvedRefsError::DecodeError {
            key: "password",
            detail: "invalid utf-8".to_string(),
        };
        assert_eq!(err.reason(), "DecodeError");
        assert!(err.message().contains("password"));
        assert!(err.message().contains("invalid utf-8"));
    }

    #[test]
    fn test_resolved_refs_error_invalid_secret_type() {
        const ALLOWED: &[&str] = &["kubernetes.io/basic-auth", "Opaque", ""];
        let err = ResolvedRefsError::InvalidSecretType {
            got: "kubernetes.io/ssh-auth".to_string(),
            allowed: ALLOWED,
        };
        assert_eq!(err.reason(), "InvalidSecretType");
        assert!(err.message().contains("ssh-auth"));
    }

    #[test]
    fn test_resolved_refs_reason_priority_invalid_secret_type_before_backend() {
        // Two errors: one Secret-content issue, one missing-resource issue.
        // `InvalidSecretType` has higher priority than `SecretNotFound` — the
        // Secret both exists and has the wrong type, so reporting "not found"
        // would mislead the operator.
        let errors = vec![
            ResolvedRefsError::SecretNotFound {
                namespace: "ns".to_string(),
                name: "other".to_string(),
            },
            ResolvedRefsError::InvalidSecretType {
                got: "kubernetes.io/tls".to_string(),
                allowed: &["kubernetes.io/basic-auth"],
            },
        ];
        let cond = resolved_refs_condition(&errors, Some(1));
        assert_eq!(cond.reason, "InvalidSecretType");
    }

    #[test]
    fn cert_content_invalid_maps_to_invalid_certificate_reason() {
        let e = ResolvedRefsError::CertContentInvalid {
            detail: "Certificate expired or not yet valid (valid: a - b)".to_string(),
        };
        assert_eq!(e.reason(), condition_reasons::INVALID_CERTIFICATE);
        assert!(e.message().contains("expired"));
        let _ = pick_resolved_refs_reason(std::slice::from_ref(&e));
    }

    #[test]
    fn test_set_parent_conditions_full_validation_and_resolved_refs_errors() {
        let mut conditions = Vec::new();
        let accepted_errors = AcceptedError::from_validation_errors(&["Secret 'ns/bad' not found".to_string()]);
        let resolved_errors = vec![ResolvedRefsError::BackendNotFound {
            namespace: "ns".to_string(),
            name: "bad".to_string(),
        }];
        set_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_errors, Some(2));

        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, "Invalid");
        assert_eq!(accepted.observed_generation, Some(2));

        let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, "BackendNotFound");
    }
}
