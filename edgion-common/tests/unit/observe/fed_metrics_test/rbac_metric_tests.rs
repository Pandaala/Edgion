use super::tests::assert_sample;
use super::*;
use metrics_exporter_prometheus::PrometheusBuilder;

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

/// Every bounded (verb, kind, source) triple has its own incrementing counter,
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
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        for verb in verbs {
            for kind in kinds {
                for source in sources {
                    record_rbac_denied(verb, kind, source);
                    record_rbac_denied(verb, kind, source);
                }
            }
        }
        let output = handle.render();
        for verb in verbs {
            for kind in kinds {
                for source in sources {
                    assert_sample(
                        &output,
                        names::RBAC_DENIED_TOTAL,
                        &[("verb", verb), ("kind", kind), ("source", source)],
                        2.0,
                    );
                }
            }
        }
    });
}

/// The kill-switch gauge follows operator intent in both directions.
#[test]
fn kill_switch_gauge_does_not_panic() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        for enabled in [true, false, true] {
            record_kill_switch_state(enabled);
            assert_sample(
                &handle.render(),
                names::KILL_SWITCH_STATE,
                &[],
                if enabled { 1.0 } else { 0.0 },
            );
        }
    });
}

/// The running gauge follows the client lifecycle independently of intent.
/// It is a separate gauge from the kill switch on purpose: a non-leader
/// replica reports kill_switch=1 (operator intent) with client_running=0.
#[test]
fn client_running_gauge_does_not_panic() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        record_kill_switch_state(true);
        for running in [true, false] {
            record_client_running(running);
            let output = handle.render();
            assert_sample(&output, names::CLIENT_RUNNING, &[], if running { 1.0 } else { 0.0 });
            assert_sample(&output, names::KILL_SWITCH_STATE, &[], 1.0);
        }
    });
}

/// The three bounded sources remain distinct even for the same verb and kind.
#[test]
fn rbac_denied_with_source_label_increments_without_panic() {
    use super::labels::rbac_source;
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        for (index, source) in [rbac_source::CENTER, rbac_source::CLI_TOKEN, rbac_source::UNKNOWN]
            .into_iter()
            .enumerate()
        {
            for _ in 0..=index {
                record_rbac_denied("watch", "HTTPRoute", source);
            }
            assert_sample(
                &handle.render(),
                names::RBAC_DENIED_TOTAL,
                &[("verb", "watch"), ("kind", "HTTPRoute"), ("source", source)],
                (index + 1) as f64,
            );
        }
    });
}
