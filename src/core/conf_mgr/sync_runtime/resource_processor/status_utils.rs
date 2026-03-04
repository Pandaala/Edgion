//! Status utilities for Gateway API resources
//!
//! Provides utilities for managing resource status according to Gateway API standards (GEP-1364):
//! - Standard conditions: Accepted, ResolvedRefs, Programmed, Ready
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

/// Create standard "ResolvedRefs" condition based on validation errors
pub fn resolved_refs_condition(validation_errors: &[String], observed_generation: Option<i64>) -> Condition {
    if validation_errors.is_empty() {
        condition_true(
            condition_types::RESOLVED_REFS,
            condition_reasons::RESOLVED_REFS,
            "All references resolved",
            observed_generation,
        )
    } else {
        let reason = infer_resolved_refs_reason(validation_errors);
        condition_false(
            condition_types::RESOLVED_REFS,
            reason,
            validation_errors.join("; "),
            observed_generation,
        )
    }
}

fn infer_resolved_refs_reason(errors: &[String]) -> &'static str {
    for err in errors {
        let lower = err.to_lowercase();
        if lower.contains("invalid") && lower.contains("kind") {
            return condition_reasons::INVALID_KIND;
        }
    }
    // Per Gateway API spec, ReferenceGrant denial takes precedence over backend-not-found.
    // Match on "referencegrant" which appears in all cross-namespace denial messages
    // (e.g. "not allowed by ReferenceGrant", "denied: NoMatchingReferenceGrant").
    for err in errors {
        let lower = err.to_lowercase();
        if lower.contains("referencegrant") {
            return condition_reasons::REF_NOT_PERMITTED;
        }
    }
    for err in errors {
        let lower = err.to_lowercase();
        if lower.contains("not found") {
            return condition_reasons::BACKEND_NOT_FOUND;
        }
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
/// `accepted_errors` controls the Accepted condition per-parent.
/// `resolved_refs_errors` controls the ResolvedRefs condition (route-level).
pub fn set_route_parent_conditions(
    conditions: &mut Vec<Condition>,
    validation_errors: &[String],
    observed_generation: Option<i64>,
) {
    set_route_parent_conditions_full(conditions, &[], validation_errors, observed_generation);
}

pub fn set_route_parent_conditions_full(
    conditions: &mut Vec<Condition>,
    accepted_errors: &[String],
    resolved_refs_errors: &[String],
    observed_generation: Option<i64>,
) {
    if accepted_errors.is_empty() {
        update_condition(conditions, accepted_condition(observed_generation));
        update_condition(conditions, programmed_condition(observed_generation));
        update_condition(conditions, ready_condition(observed_generation));
    } else {
        let reason = infer_accepted_reason(accepted_errors);
        update_condition(
            conditions,
            condition_false(
                condition_types::ACCEPTED,
                reason,
                accepted_errors.join("; "),
                observed_generation,
            ),
        );
    }
    update_condition(
        conditions,
        resolved_refs_condition(resolved_refs_errors, observed_generation),
    );
}

fn infer_accepted_reason(errors: &[String]) -> &'static str {
    // Check namespace/policy violations first (NotAllowedByListeners takes priority)
    for err in errors {
        let lower = err.to_lowercase();
        if lower.contains("not allowed") {
            return condition_reasons::NOT_ALLOWED_BY_LISTENERS;
        }
    }
    // Check for listener hostname intersection failure (NoMatchingListenerHostname)
    // This occurs when route hostnames don't intersect with any listener's hostname.
    for err in errors {
        let lower = err.to_lowercase();
        if lower.contains("listener hostname") || lower.contains("no matching hostname") {
            return condition_reasons::NO_MATCHING_LISTENER_HOSTNAME;
        }
    }
    // Check for no matching parent (sectionName or port mismatch)
    for err in errors {
        let lower = err.to_lowercase();
        if lower.contains("no matching") || lower.contains("sectionname") {
            return condition_reasons::NO_MATCHING_PARENT;
        }
    }
    condition_reasons::NO_MATCHING_PARENT
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
    fn test_resolved_refs_condition() {
        // No errors -> True
        let cond = resolved_refs_condition(&[], Some(1));
        assert_eq!(cond.status, "True");
        assert_eq!(cond.reason, "ResolvedRefs");

        // With errors -> False
        let cond = resolved_refs_condition(&["Error 1".to_string(), "Error 2".to_string()], Some(1));
        assert_eq!(cond.status, "False");
        assert_eq!(cond.reason, "RefNotPermitted");
        assert!(cond.message.contains("Error 1"));
        assert!(cond.message.contains("Error 2"));
    }
}
