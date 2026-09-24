use super::*;

fn stored(status: &str, reason: &str, message: &str, ltt: &str) -> String {
    format!(
        r#"{{"conditions":[{{"type":"Ready","status":"{status}","reason":"{reason}","message":"{message}","lastTransitionTime":"{ltt}"}}]}}"#
    )
}

#[test]
fn identical_error_skips_write() {
    let existing = stored("False", "ParseError", "bad yaml", "2026-01-01T00:00:00Z");
    assert!(
        ErrorStatus::error_for_persist("ParseError", "bad yaml", Some(&existing)).is_none(),
        "re-scanning the same broken resource must not rewrite its status"
    );
}

#[test]
fn changed_message_writes_but_preserves_ltt() {
    let existing = stored("False", "ParseError", "bad yaml at line 1", "2026-01-01T00:00:00Z");
    let status = ErrorStatus::error_for_persist("ParseError", "bad yaml at line 9", Some(&existing))
        .expect("a different message must be persisted");
    assert_eq!(
        status.conditions[0].last_transition_time, "2026-01-01T00:00:00Z",
        "status did not flip, so lastTransitionTime must be carried over"
    );
}

#[test]
fn no_prior_status_writes_fresh() {
    let status = ErrorStatus::error_for_persist("ParseError", "bad yaml", None).expect("first error must persist");
    assert_eq!(status.conditions[0].status, "False");
    assert!(!status.conditions[0].last_transition_time.is_empty());
}

#[test]
fn written_error_status_is_recognized_as_a_sidecar() {
    let persisted = serde_json::to_string(&ErrorStatus::error("ParseError", "bad yaml")).unwrap();
    assert!(
        is_error_status_sidecar_json(&persisted),
        "what write_error_status persists must be detectable as a sidecar"
    );
    let mismatch = serde_json::to_string(&ErrorStatus::error("IdentityMismatch", "key drifted")).unwrap();
    assert!(is_error_status_sidecar_json(&mismatch));
}

#[test]
fn handler_owned_status_is_never_a_sidecar() {
    // A route status carries no condition of the sidecar type, so the
    // post-repair cleanup must never delete it.
    let route_status = serde_json::json!({
        "parents": [{
            "controllerName": "edgion.io/gateway-controller",
            "conditions": [{"type": "Accepted", "status": "True", "reason": "Accepted"}]
        }]
    });
    assert!(!is_error_status_sidecar(&route_status));
    assert!(!is_error_status_sidecar(&serde_json::json!({})));
    assert!(!is_error_status_sidecar(&serde_json::Value::Null));
}

#[test]
fn unparsable_status_is_not_a_sidecar() {
    assert!(!is_error_status_sidecar_json("not json"));
}
