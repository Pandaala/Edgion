//! Status stamping for operator-authored unknown config fields.
//!
//! An Edgion-specific advisory condition, upserted on hits and **removed**
//! entirely on zero hits. Callers MUST remove the condition themselves when
//! there are no findings — an upsert-only pipeline wedges otherwise, because
//! a resource the operator fixed would keep a stale `True` forever.
//! It remains a dedicated condition rather than changing `Accepted`, because
//! unknown fields are advisory and do not block synchronization.

use super::status_utils::{condition_reasons, condition_true, condition_types, update_condition};
use edgion_resources::resources::common::Condition;

/// Cap on how many unknown-field paths are named in the condition message
/// before the rest are summarized as `(+M more)`. Kubernetes caps a
/// condition message at 32768 characters; this keeps the message small and,
/// combined with the already-sorted input, deterministic — a
/// non-deterministic message would churn the `status_changed` comparison
/// that gates inline status IO.
const MAX_LISTED_PATHS: usize = 10;

/// Build the condition message from already-sorted paths: the first
/// `MAX_LISTED_PATHS` entries, plus a deterministic `(+M more)` suffix when
/// truncated.
fn format_message(paths: &[String]) -> String {
    let shown = paths[..paths.len().min(MAX_LISTED_PATHS)]
        .iter()
        .map(|path| crate::observe::unknown_fields_registry::bounded_unknown_field_path(path))
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > MAX_LISTED_PATHS {
        format!("{shown} (+{} more)", paths.len() - MAX_LISTED_PATHS)
    } else {
        shown
    }
}

/// `Some(condition)` when `paths` is non-empty; callers MUST remove the
/// condition themselves when `None` (see the module doc comment).
pub(crate) fn unknown_fields_condition(paths: &[String], generation: Option<i64>) -> Option<Condition> {
    if paths.is_empty() {
        return None;
    }
    Some(condition_true(
        condition_types::UNKNOWN_CONFIG_FIELDS,
        condition_reasons::UNKNOWN_FIELDS_DETECTED,
        format_message(paths),
        generation,
    ))
}

/// Apply the condition (upsert on hits, retain-remove on zero hits).
pub(crate) fn apply_unknown_fields_condition(
    conditions: &mut Vec<Condition>,
    paths: &[String],
    generation: Option<i64>,
) {
    match unknown_fields_condition(paths, generation) {
        Some(c) => update_condition(conditions, c),
        None => conditions.retain(|c| c.type_ != condition_types::UNKNOWN_CONFIG_FIELDS),
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/unknown_fields_stamp_test.rs"]
mod tests;
