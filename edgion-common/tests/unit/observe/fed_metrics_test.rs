use super::labels::*;

pub(super) fn assert_sample(output: &str, name: &str, labels: &[(&str, &str)], expected: f64) {
    let line = output
        .lines()
        .find(|line| {
            line.strip_prefix(name).is_some_and(|rest| {
                (rest.starts_with('{') || rest.starts_with(' '))
                    && labels
                        .iter()
                        .all(|(key, value)| rest.contains(&format!("{key}=\"{value}\"")))
            })
        })
        .unwrap_or_else(|| panic!("missing {name} {labels:?} in:\n{output}"));
    let value: f64 = line.split_whitespace().last().unwrap().parse().unwrap();
    assert_eq!(value, expected, "{line}");
}

#[test]
fn metric_names_have_edgion_fed_prefix() {
    use super::names::*;
    for n in [
        CONNECTIONS_ACTIVE,
        CONNECTION_EVENTS_TOTAL,
        CONNECTION_DURATION_LAST,
        WATCH_EVENTS_TOTAL,
        READY_GATE_WAIT_LAST,
    ] {
        assert!(
            n.starts_with("edgion_fed_"),
            "metric {} is missing edgion_fed_ prefix",
            n
        );
    }
}

/// Helper: every label value emitted to Prometheus must use a safe
/// character set. We pick the conservative subset
/// `[A-Za-z0-9_-]` because Prometheus label *values* technically allow
/// any UTF-8, but PromQL queries, label-set serialization and
/// downstream tools are far happier with this restricted alphabet.
fn assert_label_value_charset(value: &str, origin: &str) {
    assert!(!value.is_empty(), "label value for {origin} must not be empty");
    // Conservative cap. Prometheus has no hard limit on label-value
    // length, but anything beyond ~64 chars for a *bounded enum* would
    // indicate a misuse (free-form strings should never be in `labels`).
    assert!(
        value.len() <= 64,
        "label value {value:?} for {origin} exceeds 64 chars ({} bytes)",
        value.len()
    );
    for c in value.chars() {
        assert!(
            c.is_ascii_alphanumeric() || c == '_' || c == '-',
            "label value {value:?} for {origin} contains illegal char {c:?}"
        );
    }
}

#[test]
fn all_bounded_label_values_use_safe_charset() {
    // Sweep every constant in the `labels` submodule. Adding a new
    // label without listing it here is fine for compilation, but this
    // test guards against accidental inclusion of whitespace, control
    // chars, slashes, dots, or other Prometheus-hostile characters.
    let cases: &[(&str, &str)] = &[
        ("role::CENTER", role::CENTER),
        ("role::CONTROLLER", role::CONTROLLER),
        ("event::CONNECTED", event::CONNECTED),
        ("event::DISCONNECTED", event::DISCONNECTED),
        ("event::RELOAD", event::RELOAD),
        ("direction::SENT", direction::SENT),
        ("direction::RECV", direction::RECV),
        ("rbac_source::CENTER", rbac_source::CENTER),
        ("rbac_source::CLI_TOKEN", rbac_source::CLI_TOKEN),
        ("rbac_source::UNKNOWN", rbac_source::UNKNOWN),
    ];
    for (origin, value) in cases {
        assert_label_value_charset(value, origin);
    }
}

#[test]
fn metric_names_use_safe_charset_and_reasonable_length() {
    // Prometheus metric names must match `[a-zA-Z_:][a-zA-Z0-9_:]*`.
    // We further restrict ourselves to lowercase + underscore (the
    // de-facto Prometheus convention; see metric naming guide). 64 is
    // a generous-but-finite upper bound; nothing here should exceed it.
    use super::names::*;
    let names = [
        CONNECTIONS_ACTIVE,
        CONNECTION_EVENTS_TOTAL,
        CONNECTION_DURATION_LAST,
        WATCH_EVENTS_TOTAL,
        READY_GATE_WAIT_LAST,
    ];
    for n in names {
        assert!(!n.is_empty(), "metric name must not be empty");
        assert!(n.len() <= 64, "metric name {n:?} exceeds 64 chars ({} bytes)", n.len());
        let first = n.chars().next().unwrap();
        assert!(
            first.is_ascii_lowercase() || first == '_',
            "metric name {n:?} must start with [a-z_], got {first:?}"
        );
        for c in n.chars() {
            assert!(
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_',
                "metric name {n:?} contains illegal char {c:?} \
                 (allowed: [a-z0-9_])"
            );
        }
    }
}

#[test]
fn bounded_enum_value_sets_are_exhaustive_and_unique() {
    // Pin the *full* expected set for each bounded enum. Adding a new
    // variant must be a deliberate code change here — preventing
    // silent cardinality growth from a stray `pub const`.
    fn check(name: &str, actual: &[&str], expected: &[&str]) {
        let mut a: Vec<&str> = actual.to_vec();
        let mut e: Vec<&str> = expected.to_vec();
        a.sort_unstable();
        e.sort_unstable();
        assert_eq!(a, e, "{name} value set drifted: expected {expected:?}, got {actual:?}");
        // Uniqueness: no two variants share a string value.
        let mut dedup = a.clone();
        dedup.dedup();
        assert_eq!(dedup.len(), a.len(), "{name} contains duplicate values: {actual:?}");
    }
    check("role", &[role::CENTER, role::CONTROLLER], &["center", "controller"]);
    check(
        "event",
        &[event::CONNECTED, event::DISCONNECTED, event::RELOAD],
        &["connected", "disconnected", "reload"],
    );
    check("direction", &[direction::SENT, direction::RECV], &["sent", "recv"]);
    check(
        "rbac_source",
        &[rbac_source::CENTER, rbac_source::CLI_TOKEN, rbac_source::UNKNOWN],
        &["center", "cli_token", "unknown"],
    );
}

#[test]
fn helpers_are_callable_without_recorder() {
    let emit = || {
        super::set_connections_active(role::CONTROLLER, 1);
        super::record_connection_event(role::CONTROLLER, event::CONNECTED);
        super::record_connection_duration(role::CONTROLLER, 1.5);
        super::record_watch_event("EdgionConfigData", direction::RECV);
        super::record_ready_gate_wait(0.25);
    };
    // An emission before recorder installation must not pin no-op handles.
    emit();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        emit();
        let output = handle.render();
        use super::names;
        assert_sample(&output, names::CONNECTIONS_ACTIVE, &[("role", role::CONTROLLER)], 1.0);
        assert_sample(
            &output,
            names::CONNECTION_EVENTS_TOTAL,
            &[("role", role::CONTROLLER), ("event", event::CONNECTED)],
            1.0,
        );
        assert_sample(
            &output,
            names::CONNECTION_DURATION_LAST,
            &[("role", role::CONTROLLER)],
            1.5,
        );
        assert_sample(
            &output,
            names::WATCH_EVENTS_TOTAL,
            &[("kind", "EdgionConfigData"), ("direction", direction::RECV)],
            1.0,
        );
        assert_sample(&output, names::READY_GATE_WAIT_LAST, &[], 0.25);
    });
}
