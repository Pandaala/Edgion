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

use edgion_common::time::now_rfc3339;
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
    /// The operator-authored document carries one or more keys the schema
    /// does not recognize (Edgion-specific, not a Gateway API condition).
    /// True = findings exist; the condition is removed entirely when there
    /// are none. Advisory only: the typo'd key is ignored exactly as it was
    /// before this check existed, `Accepted` is untouched, and the config
    /// still syncs to the data plane unchanged — see
    /// `skills/00-design-rules.md` (unknown-key detection).
    pub const UNKNOWN_CONFIG_FIELDS: &str = "UnknownConfigFields";
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
    // NOTE: These five reasons are **Edgion extensions**, not Gateway API
    // upstream reasons. Third-party Gateway-API tooling (Envoy Gateway / Kuma
    // dashboards, etc.) will not recognise them by name. Cross-namespace
    // denial intentionally uses the upstream `RefNotPermitted` and is *not*
    // split further — the pipeline cannot distinguish "no grant yet" from
    // "grant denies", so a more specific reason would be misleading.
    /// Edgion extension · The referenced Secret does not exist (or has not yet been cached).
    pub const SECRET_NOT_FOUND: &str = "SecretNotFound";
    /// Edgion extension · The referenced ConfigMap does not exist (or has not yet been cached).
    pub const CONFIG_MAP_NOT_FOUND: &str = "ConfigMapNotFound";
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
    /// A field value is not supported by the current implementation. Used on
    /// `Accepted` (listener fields, and HTTPRoute AI attachment / retry-cap
    /// violations that reject the whole route) and, on the same HTTPRoute path,
    /// to clear a stale `PartiallyInvalid=True` once the route serves nothing.
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
    /// Gateway-level: edgion.io/edgion-plugins annotation references a missing EdgionPlugins resource.
    pub const INVALID_PLUGINS_REF: &str = "InvalidPluginsRef";

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
    pub const INVALID_CREDENTIALS: &str = "InvalidCredentials";
    pub const ALL_RULES_VALID: &str = "AllRulesValid";

    /// Reason for UnknownConfigFields=True.
    pub const UNKNOWN_FIELDS_DETECTED: &str = "UnknownFieldsDetected";
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
    /// A supported extension was attached with an unsupported value/shape.
    UnsupportedValue { message: String },
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
            Self::UnsupportedValue { .. } => condition_reasons::UNSUPPORTED_VALUE,
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
            Self::UnsupportedValue { message } => message.clone(),
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
    /// An owning reference failed identity or supported-target validation.
    /// The typed error contains no authored values or credential material.
    InvalidReference {
        error: edgion_resources::resources::refs::ReferenceError,
    },
    /// Backend kind not supported (e.g., not "Service")
    InvalidKind { kind: String, name: String },
    /// Backend Service not found
    BackendNotFound { namespace: String, name: String },
    /// Referenced EdgionBackend not found.
    EdgionBackendNotFound { namespace: String, name: String },
    /// Referenced LinkSys credential source is absent or is not currently a
    /// valid credentialSource desired snapshot.
    LinkSysNotFound { namespace: String, name: String },
    /// Referenced Secret not found
    SecretNotFound { namespace: String, name: String },
    /// Referenced ConfigMap not found
    ConfigMapNotFound { namespace: String, name: String },
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
    /// A present ConfigData target cannot be used by this consumer. The
    /// consumer's valid base remains accepted when an overlay falls back.
    EdgionConfigDataInvalid {
        namespace: String,
        name: String,
        detail: String,
    },
    /// Edgion extension · The Secret exists but is missing one or more required
    /// `data` keys (e.g. `password` for basic-auth, `tls.key` for TLS certs).
    MissingKeys { keys: Vec<&'static str> },
    /// A SecretMatch-selected, operator-named data key is absent.
    SecretMatchKeyMissing {
        namespace: String,
        name: String,
        key: String,
    },
    /// Edgion extension · A Secret data value could not be decoded (non-UTF-8,
    /// malformed JSON, etc.). `key` identifies the offending field.
    DecodeError { key: &'static str, detail: String },
    /// A SecretMatch-selected value exists but is not usable as a non-empty UTF-8 token.
    SecretMatchValueInvalid {
        namespace: String,
        name: String,
        key: String,
        detail: &'static str,
    },
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
    /// Edgion extension · route delegation: a named child route referenced by
    /// a delegating rule (`backendRef {kind: HTTPRoute | GRPCRoute}`) does not
    /// exist. `kind` names the route kind of the missing child.
    DelegationChildNotFound {
        kind: &'static str,
        namespace: String,
        name: String,
    },
}

impl ResolvedRefsError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::InvalidReference { .. } => condition_reasons::INVALID_KIND,
            Self::InvalidKind { .. } => condition_reasons::INVALID_KIND,
            Self::BackendNotFound { .. } => condition_reasons::BACKEND_NOT_FOUND,
            Self::EdgionBackendNotFound { .. } => condition_reasons::BACKEND_NOT_FOUND,
            Self::LinkSysNotFound { .. } => condition_reasons::BACKEND_NOT_FOUND,
            Self::SecretNotFound { .. } => condition_reasons::SECRET_NOT_FOUND,
            Self::ConfigMapNotFound { .. } => condition_reasons::CONFIG_MAP_NOT_FOUND,
            Self::RefNotPermitted { .. } => condition_reasons::REF_NOT_PERMITTED,
            Self::EdgionConfigDataNotFound { .. } | Self::EdgionConfigDataInvalid { .. } => {
                condition_reasons::INVALID_EDGION_CONFIG_DATA_REF
            }
            Self::MissingKeys { .. } | Self::SecretMatchKeyMissing { .. } => condition_reasons::MISSING_KEYS,
            Self::DecodeError { .. } | Self::SecretMatchValueInvalid { .. } => condition_reasons::DECODE_ERROR,
            Self::InvalidSecretType { .. } => condition_reasons::INVALID_SECRET_TYPE,
            Self::CertContentInvalid { .. } => condition_reasons::INVALID_CERTIFICATE,
            Self::DelegationChildNotFound { .. } => condition_reasons::BACKEND_NOT_FOUND,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::InvalidReference { error } => format!("Invalid resource reference: {error}"),
            Self::InvalidKind { kind, name } => {
                format!("Invalid backend ref kind '{}' for backend '{}'", kind, name)
            }
            Self::BackendNotFound { namespace, name } => {
                format!("Service '{}/{}' not found", namespace, name)
            }
            Self::EdgionBackendNotFound { namespace, name } => {
                format!("EdgionBackend '{}/{}' not found", namespace, name)
            }
            Self::LinkSysNotFound { namespace, name } => {
                format!("LinkSys credential source '{}/{}' not found", namespace, name)
            }
            Self::SecretNotFound { namespace, name } => {
                format!("Secret '{}/{}' not found", namespace, name)
            }
            Self::ConfigMapNotFound { namespace, name } => {
                format!("ConfigMap '{}/{}' not found", namespace, name)
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
            Self::EdgionConfigDataInvalid {
                namespace,
                name,
                detail,
            } => {
                format!("EdgionConfigData '{namespace}/{name}' is unusable: {detail}")
            }
            Self::SecretMatchKeyMissing { namespace, name, key } => {
                format!("Secret '{namespace}/{name}' is missing required data key '{key}'")
            }
            Self::DecodeError { key, detail } => {
                format!("credential decode failed for key '{}': {}", key, detail)
            }
            Self::SecretMatchValueInvalid {
                namespace,
                name,
                key,
                detail,
            } => format!("Secret '{namespace}/{name}' data key '{key}' is unusable: {detail}"),
            Self::InvalidSecretType { got, allowed } => {
                format!(
                    "Secret.type {:?} is not accepted for this credential (allowed: {:?})",
                    got, allowed
                )
            }
            Self::CertContentInvalid { detail } => detail.clone(),
            Self::DelegationChildNotFound { kind, namespace, name } => {
                format!("Delegation child {} '{}/{}' not found", kind, namespace, name)
            }
        }
    }
}

// ============================================================================
// Condition helpers
// ============================================================================

// `now_rfc3339()` (the canonical operator-/K8s-facing timestamp shape) now lives in
// `edgion_common::time` so control-plane code outside this module can reuse it
// without inlining a bare `Utc::now().to_rfc3339()`. Imported at the top of the file.

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

/// Build a `PartiallyInvalid` condition. `Some(message)` → True (reason RuleInvalid)
/// listing the bad entries; `None` → False (reason AllRulesValid).
pub fn partially_invalid_condition(entry_message: Option<String>, observed_generation: Option<i64>) -> Condition {
    match entry_message {
        Some(msg) => condition_true(
            condition_types::PARTIALLY_INVALID,
            condition_reasons::RULE_INVALID,
            msg,
            observed_generation,
        ),
        None => condition_false(
            condition_types::PARTIALLY_INVALID,
            condition_reasons::ALL_RULES_VALID,
            "All entries valid",
            observed_generation,
        ),
    }
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
/// 7. SecretNotFound / ConfigMapNotFound — target credential material missing
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
            ResolvedRefsError::InvalidKind { .. } | ResolvedRefsError::InvalidReference { .. } => 0,
            ResolvedRefsError::RefNotPermitted { .. } => 1,
            ResolvedRefsError::CertContentInvalid { .. } => 2,
            ResolvedRefsError::InvalidSecretType { .. } => 3,
            ResolvedRefsError::EdgionConfigDataNotFound { .. } | ResolvedRefsError::EdgionConfigDataInvalid { .. } => 4,
            ResolvedRefsError::BackendNotFound { .. } => 5,
            ResolvedRefsError::EdgionBackendNotFound { .. } => 5,
            ResolvedRefsError::LinkSysNotFound { .. } => 5,
            // Delegation child missing is the same severity class as a missing
            // Service backend (both surface as BACKEND_NOT_FOUND).
            ResolvedRefsError::DelegationChildNotFound { .. } => 5,
            ResolvedRefsError::SecretNotFound { .. } | ResolvedRefsError::ConfigMapNotFound { .. } => 6,
            ResolvedRefsError::DecodeError { .. } | ResolvedRefsError::SecretMatchValueInvalid { .. } => 7,
            ResolvedRefsError::MissingKeys { .. } | ResolvedRefsError::SecretMatchKeyMissing { .. } => 8,
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
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/status_utils_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/support/conf_mgr/sync_runtime/resource_processor/status_utils.rs"]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::set_parent_conditions;
