use super::*;

/// Name constants for the RBAC metrics must follow the `edgion_fed_` prefix
/// convention and use only lowercase ASCII + underscores.
#[test]
fn rbac_metric_names_are_stable_and_valid() {
    assert_eq!(names::RBAC_DENIED_TOTAL, "edgion_fed_rbac_denied_total");
    assert_eq!(names::KILL_SWITCH_STATE, "edgion_fed_kill_switch_state");
    assert_eq!(names::CLIENT_RUNNING, "edgion_fed_client_running");

    for n in [
        names::RBAC_DENIED_TOTAL,
        names::KILL_SWITCH_STATE,
        names::CLIENT_RUNNING,
    ] {
        assert!(n.starts_with("edgion_fed_"), "metric {n} must have edgion_fed_ prefix");
        assert!(!n.is_empty(), "metric name must not be empty");
        let first = n.chars().next().unwrap();
        assert!(
            first.is_ascii_lowercase() || first == '_',
            "metric {n} must start with [a-z_]"
        );
        for c in n.chars() {
            assert!(
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_',
                "metric name {n} contains illegal char {c:?} (allowed: [a-z0-9_])"
            );
        }
    }
}

/// `record_rbac_denied` must not panic for any bounded (verb, kind, source) triple,
/// including the sentinel "unknown" values used for unclassified requests.
#[test]
fn rbac_denied_counter_increments_without_panic() {
    // Bounded verb values produced by Verb::as_str() + the "unknown" sentinel.
    let verbs = [
        "get",
        "list",
        "list-keys",
        "watch",
        "create",
        "update",
        "delete",
        "failover",
        "acme-trigger",
        "conf-sync-rotate",
        "reload",
        "server-info",
        "diagnostics",
        "wipe-all",
        "debug",
        "unknown",
    ];
    // Bounded kind values: known kinds, synthetic labels, "unknown".
    let kinds = ["Secret", "EdgionConfigData", "HTTPRoute", "RegionRoute", "*", "unknown"];
    // Bounded source values from labels::rbac_source.
    let sources = [
        labels::rbac_source::CENTER,
        labels::rbac_source::CLI_TOKEN,
        labels::rbac_source::UNKNOWN,
    ];
    for verb in verbs {
        for kind in kinds {
            for source in sources {
                // Must not panic; the metrics crate no-op recorder handles absent recorder.
                record_rbac_denied(verb, kind, source);
            }
        }
    }
}

/// `record_kill_switch_state` must not panic for both `true` and `false`.
#[test]
fn kill_switch_gauge_does_not_panic() {
    record_kill_switch_state(true);
    record_kill_switch_state(false);
    // Toggle back to enabled to leave gauge in a known state.
    record_kill_switch_state(true);
}

/// `record_client_running` must not panic for both `true` and `false`.
/// It is a separate gauge from the kill switch on purpose: a non-leader
/// replica reports kill_switch=1 (operator intent) with client_running=0.
#[test]
fn client_running_gauge_does_not_panic() {
    record_client_running(true);
    record_client_running(false);
}

/// `record_rbac_denied` with the `source` label must not panic for
/// the cli_token source, confirming the three bounded source values are usable.
#[test]
fn rbac_denied_with_source_label_increments_without_panic() {
    use super::labels::rbac_source;
    // The canonical case: a cli token denied on Secret.
    record_rbac_denied("get", "Secret", rbac_source::CLI_TOKEN);
    record_rbac_denied("list", "EdgionConfigData", rbac_source::CENTER);
    record_rbac_denied("unknown", "unknown", rbac_source::UNKNOWN);
    // All three bounded source values work.
    for source in [rbac_source::CENTER, rbac_source::CLI_TOKEN, rbac_source::UNKNOWN] {
        record_rbac_denied("watch", "HTTPRoute", source);
    }
}
