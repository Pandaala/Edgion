//! Status utilities for Gateway API resources
//!
//! Provides utilities for managing resource status according to Gateway API standards (GEP-1364):
//! - Standard conditions: Accepted, ResolvedRefs, Programmed, Ready
//! - Typed error enums for compile-time safe reason mapping
//! - Condition update helpers
//! - Time formatting

use crate::types::resources::common::Condition;
use chrono::{SecondsFormat, Utc};

/// Standard Gateway API condition types
pub mod condition_types {
    /// Resource is syntactically and semantically valid
    pub const ACCEPTED: &str = "Accepted";
    /// All references to other objects are resolved
    pub const RESOLVED_REFS: &str = "ResolvedRefs";
    /// Configuration has been sent to the data plane
    pub const PROGRAMMED: &str = "Programmed";
    /// Data plane is ready to serve traffic
    pub const READY: &str = "Ready";
    /// Listener conflicts with another Listener (port/hostname collision)
    pub const CONFLICTED: &str = "Conflicted";
    /// Gateway has listeners that are not valid (used at Gateway level)
    pub const LISTENERS_NOT_VALID: &str = "ListenersNotValid";
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

    // ResolvedRefs reasons
    pub const RESOLVED_REFS: &str = "ResolvedRefs";
    pub const REF_NOT_PERMITTED: &str = "RefNotPermitted";
    pub const BACKEND_NOT_FOUND: &str = "BackendNotFound";
    pub const INVALID_KIND: &str = "InvalidKind";

    // Programmed reasons
    pub const PROGRAMMED: &str = "Programmed";
    pub const INVALID: &str = "Invalid";

    // Ready reasons
    pub const READY: &str = "Ready";
    pub const PENDING: &str = "Pending";

    // Conflicted reasons (for Listener port conflicts)
    pub const LISTENER_CONFLICT: &str = "ListenerConflict";
    pub const NO_CONFLICTS: &str = "NoConflicts";
}

// ============================================================================
// Typed error enums — replace string-based reason inference
// ============================================================================

/// Typed errors for Route Accepted condition.
///
/// Each variant maps to a specific Gateway API reason string, eliminating
/// string-based inference and providing compile-time safety.
#[derive(Debug, Clone, PartialEq)]
pub enum AcceptedError {
    /// Route namespace not allowed by listener's allowedRoutes policy
    NotAllowedByListeners { route_ns: String },
    /// Route hostnames don't intersect with any listener hostname
    NoMatchingListenerHostname { hostnames: Vec<String> },
    /// sectionName doesn't match any listener
    NoMatchingParent { section_name: String },
}

impl AcceptedError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::NotAllowedByListeners { .. } => condition_reasons::NOT_ALLOWED_BY_LISTENERS,
            Self::NoMatchingListenerHostname { .. } => condition_reasons::NO_MATCHING_LISTENER_HOSTNAME,
            Self::NoMatchingParent { .. } => condition_reasons::NO_MATCHING_PARENT,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NotAllowedByListeners { route_ns } => {
                format!("Route namespace '{}' not allowed by Gateway listeners", route_ns)
            }
            Self::NoMatchingListenerHostname { hostnames } => {
                format!("No matching hostname for route hostnames {:?}", hostnames)
            }
            Self::NoMatchingParent { section_name } => {
                format!("No matching listener for sectionName '{}'", section_name)
            }
        }
    }
}

/// Typed errors for Route ResolvedRefs condition.
///
/// Each variant maps to a specific Gateway API reason string, eliminating
/// string-based inference and providing compile-time safety.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedRefsError {
    /// Backend kind not supported (e.g., not "Service")
    InvalidKind { kind: String, name: String },
    /// Backend Service not found
    BackendNotFound { namespace: String, name: String },
    /// Cross-namespace reference denied by ReferenceGrant policy
    RefNotPermitted {
        target_namespace: String,
        target_name: String,
    },
}

impl ResolvedRefsError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::InvalidKind { .. } => condition_reasons::INVALID_KIND,
            Self::BackendNotFound { .. } => condition_reasons::BACKEND_NOT_FOUND,
            Self::RefNotPermitted { .. } => condition_reasons::REF_NOT_PERMITTED,
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
            Self::RefNotPermitted {
                target_namespace,
                target_name,
            } => {
                format!(
                    "Cross-namespace reference to {}/{} not allowed by ReferenceGrant",
                    target_namespace, target_name
                )
            }
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
    accepted_condition_with_message(observed_generation, "Route accepted")
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
/// Priority order (matching Gateway API spec precedence):
/// 1. InvalidKind — fundamentally wrong backend type
/// 2. RefNotPermitted — policy denial
/// 3. BackendNotFound — missing backend
fn pick_resolved_refs_reason(errors: &[ResolvedRefsError]) -> &'static str {
    if errors
        .iter()
        .any(|e| matches!(e, ResolvedRefsError::InvalidKind { .. }))
    {
        return condition_reasons::INVALID_KIND;
    }
    if errors
        .iter()
        .any(|e| matches!(e, ResolvedRefsError::RefNotPermitted { .. }))
    {
        return condition_reasons::REF_NOT_PERMITTED;
    }
    if errors
        .iter()
        .any(|e| matches!(e, ResolvedRefsError::BackendNotFound { .. }))
    {
        return condition_reasons::BACKEND_NOT_FOUND;
    }
    condition_reasons::REF_NOT_PERMITTED
}

/// Create standard "Programmed: True" condition
pub fn programmed_condition(observed_generation: Option<i64>) -> Condition {
    condition_true(
        condition_types::PROGRAMMED,
        condition_reasons::PROGRAMMED,
        "Configuration programmed",
        observed_generation,
    )
}

/// Create standard "Ready: True" condition
pub fn ready_condition(observed_generation: Option<i64>) -> Condition {
    condition_true(
        condition_types::READY,
        condition_reasons::READY,
        "Route is ready",
        observed_generation,
    )
}

/// Set all standard conditions for a route's parent status.
/// Uses empty `accepted_errors`; `resolved_refs_errors` controls the ResolvedRefs condition.
pub fn set_route_parent_conditions(
    conditions: &mut Vec<Condition>,
    resolved_refs_errors: &[ResolvedRefsError],
    observed_generation: Option<i64>,
) {
    set_route_parent_conditions_full(conditions, &[], resolved_refs_errors, observed_generation);
}

/// Set all standard conditions for a route's parent status.
/// `accepted_errors` controls the Accepted condition per-parent.
/// `resolved_refs_errors` controls the ResolvedRefs condition (route-level).
pub fn set_route_parent_conditions_full(
    conditions: &mut Vec<Condition>,
    accepted_errors: &[AcceptedError],
    resolved_refs_errors: &[ResolvedRefsError],
    observed_generation: Option<i64>,
) {
    if accepted_errors.is_empty() {
        update_condition(conditions, accepted_condition(observed_generation));
        update_condition(conditions, programmed_condition(observed_generation));
        update_condition(conditions, ready_condition(observed_generation));
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
            route_ns: "test-ns".to_string(),
        };
        assert_eq!(err.reason(), "NotAllowedByListeners");
        assert_eq!(
            err.message(),
            "Route namespace 'test-ns' not allowed by Gateway listeners"
        );
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
            target_namespace: "other-ns".to_string(),
            target_name: "other-svc".to_string(),
        };
        assert_eq!(err.reason(), "RefNotPermitted");
        assert_eq!(
            err.message(),
            "Cross-namespace reference to other-ns/other-svc not allowed by ReferenceGrant"
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
    fn test_set_route_parent_conditions_no_errors() {
        let mut conditions = Vec::new();
        set_route_parent_conditions(&mut conditions, &[], Some(1));

        assert_eq!(conditions.len(), 4); // Accepted, Programmed, Ready, ResolvedRefs
        assert!(conditions.iter().all(|c| c.status == "True"));
        assert!(conditions.iter().any(|c| c.type_ == "Accepted"));
        assert!(conditions.iter().any(|c| c.type_ == "Programmed"));
        assert!(conditions.iter().any(|c| c.type_ == "Ready"));
        assert!(conditions.iter().any(|c| c.type_ == "ResolvedRefs"));
    }

    #[test]
    fn test_set_route_parent_conditions_with_resolved_refs_errors() {
        let mut conditions = Vec::new();
        let errors = vec![ResolvedRefsError::RefNotPermitted {
            target_namespace: "ns".to_string(),
            target_name: "svc".to_string(),
        }];
        set_route_parent_conditions(&mut conditions, &errors, Some(1));

        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "True");

        let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, "RefNotPermitted");
    }

    #[test]
    fn test_set_route_parent_conditions_full_with_accepted_errors() {
        let mut conditions = Vec::new();
        let accepted_errors = vec![AcceptedError::NotAllowedByListeners {
            route_ns: "blocked-ns".to_string(),
        }];
        set_route_parent_conditions_full(&mut conditions, &accepted_errors, &[], Some(1));

        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, "NotAllowedByListeners");
        assert!(accepted.message.contains("blocked-ns"));

        // Programmed and Ready should NOT be set when Accepted is False
        assert!(!conditions.iter().any(|c| c.type_ == "Programmed"));
        assert!(!conditions.iter().any(|c| c.type_ == "Ready"));

        let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        assert_eq!(resolved.status, "True");
    }

    #[test]
    fn test_set_route_parent_conditions_full_with_both_errors() {
        let mut conditions = Vec::new();
        let accepted_errors = vec![AcceptedError::NoMatchingParent {
            section_name: "nonexistent".to_string(),
        }];
        let resolved_errors = vec![ResolvedRefsError::BackendNotFound {
            namespace: "default".to_string(),
            name: "missing-svc".to_string(),
        }];
        set_route_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_errors, Some(1));

        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, "NoMatchingParent");

        let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, "BackendNotFound");
    }
}
