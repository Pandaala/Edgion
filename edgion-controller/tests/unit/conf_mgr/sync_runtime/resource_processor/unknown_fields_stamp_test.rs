use super::*;

#[test]
fn no_paths_yields_no_condition() {
    assert!(unknown_fields_condition(&[], Some(1)).is_none());
}

#[test]
fn paths_yield_true_condition_with_joined_message() {
    let paths = vec!["config.denys".to_string(), "requestPlugins[0].config.typo".to_string()];
    let c = unknown_fields_condition(&paths, Some(3)).expect("hits must produce a condition");
    assert_eq!(c.type_, condition_types::UNKNOWN_CONFIG_FIELDS);
    assert_eq!(c.reason, condition_reasons::UNKNOWN_FIELDS_DETECTED);
    assert_eq!(c.status, "True");
    assert_eq!(c.message, "config.denys, requestPlugins[0].config.typo");
    assert_eq!(c.observed_generation, Some(3));
}

#[test]
fn message_caps_at_max_listed_paths() {
    let paths: Vec<String> = (0..13).map(|i| format!("path{i}")).collect();
    let c = unknown_fields_condition(&paths, None).unwrap();
    let expected_shown = paths[..MAX_LISTED_PATHS].join(", ");
    assert_eq!(c.message, format!("{expected_shown} (+3 more)"));
}

#[test]
fn message_bounds_individual_paths_without_changing_omitted_count() {
    let paths: Vec<String> = (0..27).map(|i| format!("path{i}.{}", "é".repeat(600))).collect();
    let c = unknown_fields_condition(&paths, None).unwrap();
    assert!(c.message.ends_with("(+17 more)"));
    assert!(c.message.contains("bytes)"));
}

#[test]
fn apply_upserts_then_removes_on_fix() {
    let mut conditions = Vec::new();
    apply_unknown_fields_condition(&mut conditions, &["config.denys".to_string()], Some(1));
    assert_eq!(conditions.len(), 1);
    assert_eq!(conditions[0].type_, condition_types::UNKNOWN_CONFIG_FIELDS);
    assert_eq!(conditions[0].status, "True");

    // Operator fixes the typo: the next apply must REMOVE the stale
    // condition, not leave it upserted at a frozen message (the wedge
    // trap the module doc comment warns about).
    apply_unknown_fields_condition(&mut conditions, &[], Some(2));
    assert!(
        conditions.is_empty(),
        "condition must be removed once findings are empty, got {conditions:?}"
    );
}

/// Repeating the same apply must be a semantic no-op. This is the loop
/// guard: in Kubernetes mode a status write produces a watch event that
/// re-enters the workqueue, and the ONLY thing stopping an endless
/// write->event->write cycle is that the second pass computes an
/// identical status, so `status_semantically_changed` says no and no
/// write is issued. A wall-clock value, an unstable ordering, or a
/// `last_transition_time` bumped on every apply would each turn this into
/// a permanent hot loop against the API server, so pin it explicitly
/// rather than trusting the mechanism to stay correct.
#[test]
fn repeated_apply_with_same_findings_is_a_semantic_no_op() {
    let paths = vec!["config.denys".to_string(), "config.alow".to_string()];

    let mut first = Vec::new();
    apply_unknown_fields_condition(&mut first, &paths, Some(7));
    let first_json = serde_json::to_string(&first).expect("conditions serialize");

    // Second pass over an unchanged resource: same findings, same
    // generation, and `first` already carries the condition.
    let mut second = first.clone();
    apply_unknown_fields_condition(&mut second, &paths, Some(7));
    let second_json = serde_json::to_string(&second).expect("conditions serialize");

    assert_eq!(
        first_json, second_json,
        "a repeated apply must not change the conditions at all, including last_transition_time"
    );

    // And the status-diff gate that actually decides whether a write
    // happens must agree that nothing changed.
    let old_status = format!(r#"{{"conditions":{first_json}}}"#);
    let new_status = format!(r#"{{"conditions":{second_json}}}"#);
    assert!(
        !crate::conf_mgr::sync_runtime::resource_processor::status_diff::status_semantically_changed(
            "EdgionPlugins",
            &old_status,
            &new_status,
            None,
        ),
        "status_semantically_changed must report no change for a repeated apply"
    );
}

#[test]
fn apply_leaves_other_conditions_untouched() {
    let mut conditions = vec![condition_true(
        condition_types::ACCEPTED,
        condition_reasons::ACCEPTED,
        "Resource accepted",
        Some(1),
    )];
    apply_unknown_fields_condition(&mut conditions, &["config.denys".to_string()], Some(1));
    assert_eq!(conditions.len(), 2);
    apply_unknown_fields_condition(&mut conditions, &[], Some(2));
    assert_eq!(conditions.len(), 1);
    assert_eq!(conditions[0].type_, condition_types::ACCEPTED);
}
