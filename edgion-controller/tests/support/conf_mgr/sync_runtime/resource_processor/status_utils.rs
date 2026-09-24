use super::*;

/// Set standard conditions (Accepted, ResolvedRefs) for a resource's parent status.
/// Uses empty `accepted_errors`; `resolved_refs_errors` controls the ResolvedRefs condition.
/// Test-only: production handlers call `set_parent_conditions_full` directly.
#[cfg(test)]
pub fn set_parent_conditions(
    conditions: &mut Vec<Condition>,
    resolved_refs_errors: &[ResolvedRefsError],
    observed_generation: Option<i64>,
) {
    set_parent_conditions_full(conditions, &[], resolved_refs_errors, observed_generation);
}
