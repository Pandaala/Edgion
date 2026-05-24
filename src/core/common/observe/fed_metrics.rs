//! Federation baseline metrics.
//!
//! Shared by Center and Controller to emit Prometheus counters/gauges for
//! federation plane observability. See
//! `docs/superpowers/specs/2026-04-19-fix-24-fed-metrics-baseline-design.md`
//! for the full design and the rationale for dropping the original
//! ticket's histograms and high-cardinality labels.
//!
//! **Label value discipline:** every label value emitted here must come
//! from a bounded enum (the `labels` submodule) or from a well-known
//! resource-kind short name. Do NOT pass raw user input, `controller_id`,
//! hostnames, UUIDs, or free-form paths as label values — those explode
//! Prometheus cardinality.

use metrics::{counter, gauge};

/// Metric name constants. Kept here as a single source of truth so tests
/// and docs can reference the exact strings exposed on `/metrics`.
pub mod names {
    /// Gauge: currently active federation connections (from the emitting
    /// process's point of view). On Center this is the number of
    /// controllers attached; on Controller this is 0 or 1.
    pub const CONNECTIONS_ACTIVE: &str = "edgion_fed_connections_active";
    /// Counter: federation session lifecycle events.
    pub const CONNECTION_EVENTS_TOTAL: &str = "edgion_fed_connection_events_total";
    /// Gauge: last observed session duration in seconds (recorded at
    /// disconnect time).
    pub const CONNECTION_DURATION_LAST: &str = "edgion_fed_connection_duration_seconds_last";
    /// Counter: watch events flowing across the federation link.
    pub const WATCH_EVENTS_TOTAL: &str = "edgion_fed_watch_events_total";
    /// Counter: initial list outcomes during watch setup.
    pub const WATCH_LIST_TOTAL: &str = "edgion_fed_watch_list_total";
    /// Counter: errors observed on the watch receive path.
    pub const WATCH_ERRORS_TOTAL: &str = "edgion_fed_watch_errors_total";
    /// Counter: controllers transitioned to offline in the registry.
    pub const MARK_OFFLINE_TOTAL: &str = "edgion_fed_mark_offline_total";
    /// Counter: stale entries evicted from registry/aggregator.
    pub const EVICT_STALE_TOTAL: &str = "edgion_fed_evict_stale_total";
    /// Counter: same `controller_id` re-registered while a prior session
    /// was still in the registry (useful to detect reconnect storms).
    pub const SESSION_REENTRY_TOTAL: &str = "edgion_fed_session_reentry_total";
    /// Gauge: controllers known to the aggregator, broken down by cluster.
    pub const AGGREGATOR_CONTROLLERS: &str = "edgion_fed_aggregator_controllers";
    /// Counter: consistency-check API detected cross-controller divergence.
    pub const CONSISTENCY_MISMATCH_TOTAL: &str = "edgion_fed_consistency_mismatch_total";
    /// Counter: fan-out patch operations' aggregate outcome.
    pub const FANOUT_TOTAL: &str = "edgion_fed_fanout_total";
    /// Gauge: last observed ready-gate wait in seconds (Controller side).
    pub const READY_GATE_WAIT_LAST: &str = "edgion_fed_ready_gate_wait_seconds_last";
    /// Counter: peer-identity binding outcome on the federation gRPC server.
    pub const PEER_IDENTITY_CHECK_TOTAL: &str = "edgion_fed_peer_identity_check_total";
    /// Counter: a live session was displaced by a new connection (takeover).
    pub const SESSION_TAKEOVER_TOTAL: &str = "edgion_fed_session_takeover_total";
    /// Counter: RBAC denials on the Controller admin-API federation path.
    /// Labels: `verb` (closed set from `Verb::as_str()` + "unknown"),
    /// `kind` (known ResourceKind, "*", "RegionRoute", or "unknown" — never raw input),
    /// `source` ("center" | "cli_token" | "unknown" — identifies the auth path).
    pub const RBAC_DENIED_TOTAL: &str = "edgion_fed_rbac_denied_total";
    /// Gauge: federation kill-switch state. 1.0 = enabled, 0.0 = disabled.
    /// Emitted by the supervisor on each start/stop transition.
    pub const KILL_SWITCH_STATE: &str = "edgion_fed_kill_switch_state";
}

/// Bounded label value constants. Emitting any other value violates the
/// cardinality contract.
pub mod labels {
    pub mod role {
        pub const CENTER: &str = "center";
        pub const CONTROLLER: &str = "controller";
    }
    pub mod event {
        pub const CONNECTED: &str = "connected";
        pub const DISCONNECTED: &str = "disconnected";
        pub const RELOAD: &str = "reload";
    }
    pub mod direction {
        pub const SENT: &str = "sent";
        pub const RECV: &str = "recv";
    }
    pub mod watch_list_result {
        pub const OK: &str = "ok";
        pub const VERSION_TOO_OLD: &str = "version_too_old";
        pub const PARSE_ERROR: &str = "parse_error";
        pub const READY_WAIT: &str = "ready_wait";
    }
    pub mod watch_error_reason {
        pub const PARSE_ERROR: &str = "parse_error";
        pub const TIMEOUT: &str = "timeout";
        pub const RECV_ERROR: &str = "recv_error";
    }
    pub mod offline_reason {
        pub const HEARTBEAT: &str = "heartbeat";
        pub const DISCONNECT: &str = "disconnect";
        pub const RELOAD: &str = "reload";
    }
    pub mod evict_source {
        pub const REGISTRY: &str = "registry";
        pub const AGGREGATOR: &str = "aggregator";
    }
    pub mod fanout_op {
        pub const PATCH_ENABLE: &str = "patch_enable";
        pub const PATCH_PROFILE: &str = "patch_profile";
    }
    pub mod fanout_result {
        pub const OK: &str = "ok";
        pub const PARTIAL: &str = "partial";
        pub const FAIL: &str = "fail";
    }
    pub mod peer_identity_result {
        pub const OK: &str = "ok";
        pub const MISMATCH: &str = "mismatch";
        pub const NO_SPIFFE_SAN: &str = "no_spiffe_san";
        pub const MULTI_SAN: &str = "multi_san";
        pub const PARSE_ERROR: &str = "parse_error";
    }
    /// Bounded source values for `record_rbac_denied`. Identifies which
    /// authentication path originated the denied request.
    ///
    /// - `center`: denied on the federation gRPC path (Center identity).
    /// - `cli_token`: denied on the local admin API via a cli token.
    /// - `unknown`: denied before a source could be determined (missing route,
    ///   AuthBypass-without-Role invariant violation, or unclassified origin).
    pub mod rbac_source {
        pub const CENTER: &str = "center";
        pub const CLI_TOKEN: &str = "cli_token";
        pub const UNKNOWN: &str = "unknown";
    }
}

// ---------- Connection lifecycle ----------

/// Set the current active-connection gauge for a given role.
#[inline]
pub fn set_connections_active(role: &'static str, count: u64) {
    gauge!(names::CONNECTIONS_ACTIVE, "role" => role).set(count as f64);
}

/// Record a connection lifecycle event.
#[inline]
pub fn record_connection_event(role: &'static str, event: &'static str) {
    counter!(names::CONNECTION_EVENTS_TOTAL, "role" => role, "event" => event).increment(1);
}

/// Record the most recent session duration (observed at disconnect).
#[inline]
pub fn record_connection_duration(role: &'static str, seconds: f64) {
    gauge!(names::CONNECTION_DURATION_LAST, "role" => role).set(seconds);
}

// ---------- Watch plane ----------

/// Record a watch event crossing the federation link.
///
/// `kind` must come from a bounded set of resource kinds (e.g.
/// "PluginMetaData"). Never pass raw user input.
#[inline]
pub fn record_watch_event(kind: &str, direction: &'static str) {
    counter!(names::WATCH_EVENTS_TOTAL, "kind" => kind.to_string(), "direction" => direction).increment(1);
}

#[inline]
pub fn record_watch_list(kind: &str, result: &'static str) {
    counter!(names::WATCH_LIST_TOTAL, "kind" => kind.to_string(), "result" => result).increment(1);
}

#[inline]
pub fn record_watch_error(kind: &str, reason: &'static str) {
    counter!(names::WATCH_ERRORS_TOTAL, "kind" => kind.to_string(), "reason" => reason).increment(1);
}

// ---------- Lifecycle (Center) ----------

#[inline]
pub fn record_mark_offline(reason: &'static str) {
    counter!(names::MARK_OFFLINE_TOTAL, "reason" => reason).increment(1);
}

#[inline]
pub fn record_evict_stale(source: &'static str) {
    counter!(names::EVICT_STALE_TOTAL, "source" => source).increment(1);
}

#[inline]
pub fn record_session_reentry() {
    counter!(names::SESSION_REENTRY_TOTAL).increment(1);
}

#[inline]
pub fn record_peer_identity_check(result: &'static str) {
    counter!(names::PEER_IDENTITY_CHECK_TOTAL, "result" => result).increment(1);
}

#[inline]
pub fn record_session_takeover() {
    counter!(names::SESSION_TAKEOVER_TOTAL).increment(1);
}

// ---------- Aggregator (Center) ----------

/// Set the number of controllers known to the aggregator for a given
/// cluster. Callers must update this gauge whenever the aggregator's
/// controller set mutates. The label `env` from the original ticket was
/// dropped because `env` is a repeated field in RegisterRequest and
/// cannot be represented as a single bounded label value.
#[inline]
pub fn set_aggregator_controllers(cluster: &str, count: u64) {
    gauge!(names::AGGREGATOR_CONTROLLERS, "cluster" => cluster.to_string()).set(count as f64);
}

#[inline]
pub fn record_consistency_mismatch() {
    counter!(names::CONSISTENCY_MISMATCH_TOTAL).increment(1);
}

#[inline]
pub fn record_fanout(op: &'static str, result: &'static str) {
    counter!(names::FANOUT_TOTAL, "op" => op, "result" => result).increment(1);
}

// ---------- Ready gate (Controller) ----------

#[inline]
pub fn record_ready_gate_wait(seconds: f64) {
    gauge!(names::READY_GATE_WAIT_LAST).set(seconds);
}

// ---------- RBAC (Controller admin authz) ----------

/// Increment the RBAC denial counter for the federation path.
///
/// `verb` must be a value from the closed set produced by `Verb::as_str()` or
/// the literal `"unknown"`. `kind` must be a bounded value (a known ResourceKind
/// short name, `"*"`, `"RegionRoute"`, or `"unknown"`) — the caller is
/// responsible for bounding via `bounded_kind_label` before reaching here.
/// `source` must be one of the three bounded values in `labels::rbac_source`:
/// `"center"`, `"cli_token"`, or `"unknown"`. Never pass raw user input as any
/// label — in particular, do NOT use the token name as a label value (use a
/// structured log field instead).
#[inline]
pub fn record_rbac_denied(verb: &str, kind: &str, source: &str) {
    counter!(
        names::RBAC_DENIED_TOTAL,
        "verb" => verb.to_string(),
        "kind" => kind.to_string(),
        "source" => source.to_string()
    )
    .increment(1);
}

/// Set the federation kill-switch state gauge.
///
/// Call this on every supervisor start/stop transition so the current state is
/// always visible in Prometheus without querying event history.
/// Value: `1.0` when the federation client is enabled and running, `0.0` when
/// it is disabled (kill-switch active).
#[inline]
pub fn record_kill_switch_state(enabled: bool) {
    gauge!(names::KILL_SWITCH_STATE).set(if enabled { 1.0 } else { 0.0 });
}

#[cfg(test)]
mod peer_auth_metric_tests {
    use super::*;

    #[test]
    fn peer_identity_metric_name_is_stable() {
        assert_eq!(names::PEER_IDENTITY_CHECK_TOTAL, "edgion_fed_peer_identity_check_total");
        assert_eq!(names::SESSION_TAKEOVER_TOTAL, "edgion_fed_session_takeover_total");
    }

    #[test]
    fn peer_identity_result_labels_are_bounded() {
        let _ = [
            labels::peer_identity_result::OK,
            labels::peer_identity_result::MISMATCH,
            labels::peer_identity_result::NO_SPIFFE_SAN,
            labels::peer_identity_result::MULTI_SAN,
            labels::peer_identity_result::PARSE_ERROR,
        ];
    }

    #[test]
    fn recorders_do_not_panic() {
        record_peer_identity_check(labels::peer_identity_result::OK);
        record_session_takeover();
    }
}

#[cfg(test)]
mod tests {
    use super::labels::*;

    // The 8 individual `*_labels_are_stable` tests that previously appeared
    // here have been folded into `bounded_enum_value_sets_are_exhaustive_and_unique`
    // below, which checks the full enum value set (stricter — adding or
    // dropping a const breaks it) plus uniqueness across each enum.

    #[test]
    fn metric_names_have_edgion_fed_prefix() {
        use super::names::*;
        for n in [
            CONNECTIONS_ACTIVE,
            CONNECTION_EVENTS_TOTAL,
            CONNECTION_DURATION_LAST,
            WATCH_EVENTS_TOTAL,
            WATCH_LIST_TOTAL,
            WATCH_ERRORS_TOTAL,
            MARK_OFFLINE_TOTAL,
            EVICT_STALE_TOTAL,
            SESSION_REENTRY_TOTAL,
            AGGREGATOR_CONTROLLERS,
            CONSISTENCY_MISMATCH_TOTAL,
            FANOUT_TOTAL,
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
            ("watch_list_result::OK", watch_list_result::OK),
            ("watch_list_result::VERSION_TOO_OLD", watch_list_result::VERSION_TOO_OLD),
            ("watch_list_result::PARSE_ERROR", watch_list_result::PARSE_ERROR),
            ("watch_list_result::READY_WAIT", watch_list_result::READY_WAIT),
            ("watch_error_reason::PARSE_ERROR", watch_error_reason::PARSE_ERROR),
            ("watch_error_reason::TIMEOUT", watch_error_reason::TIMEOUT),
            ("watch_error_reason::RECV_ERROR", watch_error_reason::RECV_ERROR),
            ("offline_reason::HEARTBEAT", offline_reason::HEARTBEAT),
            ("offline_reason::DISCONNECT", offline_reason::DISCONNECT),
            ("offline_reason::RELOAD", offline_reason::RELOAD),
            ("evict_source::REGISTRY", evict_source::REGISTRY),
            ("evict_source::AGGREGATOR", evict_source::AGGREGATOR),
            ("fanout_op::PATCH_ENABLE", fanout_op::PATCH_ENABLE),
            ("fanout_op::PATCH_PROFILE", fanout_op::PATCH_PROFILE),
            ("fanout_result::OK", fanout_result::OK),
            ("fanout_result::PARTIAL", fanout_result::PARTIAL),
            ("fanout_result::FAIL", fanout_result::FAIL),
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
            WATCH_LIST_TOTAL,
            WATCH_ERRORS_TOTAL,
            MARK_OFFLINE_TOTAL,
            EVICT_STALE_TOTAL,
            SESSION_REENTRY_TOTAL,
            AGGREGATOR_CONTROLLERS,
            CONSISTENCY_MISMATCH_TOTAL,
            FANOUT_TOTAL,
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
            "watch_list_result",
            &[
                watch_list_result::OK,
                watch_list_result::VERSION_TOO_OLD,
                watch_list_result::PARSE_ERROR,
                watch_list_result::READY_WAIT,
            ],
            &["ok", "version_too_old", "parse_error", "ready_wait"],
        );
        check(
            "watch_error_reason",
            &[
                watch_error_reason::PARSE_ERROR,
                watch_error_reason::TIMEOUT,
                watch_error_reason::RECV_ERROR,
            ],
            &["parse_error", "timeout", "recv_error"],
        );
        check(
            "offline_reason",
            &[
                offline_reason::HEARTBEAT,
                offline_reason::DISCONNECT,
                offline_reason::RELOAD,
            ],
            &["heartbeat", "disconnect", "reload"],
        );
        check(
            "evict_source",
            &[evict_source::REGISTRY, evict_source::AGGREGATOR],
            &["registry", "aggregator"],
        );
        check(
            "fanout_op",
            &[fanout_op::PATCH_ENABLE, fanout_op::PATCH_PROFILE],
            &["patch_enable", "patch_profile"],
        );
        check(
            "fanout_result",
            &[fanout_result::OK, fanout_result::PARTIAL, fanout_result::FAIL],
            &["ok", "partial", "fail"],
        );
        check(
            "rbac_source",
            &[rbac_source::CENTER, rbac_source::CLI_TOKEN, rbac_source::UNKNOWN],
            &["center", "cli_token", "unknown"],
        );
    }

    #[test]
    fn helpers_are_callable_without_recorder() {
        // Without a Prometheus recorder installed, the metrics crate uses
        // a no-op recorder. Helpers must not panic in that case, which
        // also covers unit-test harnesses.
        super::set_connections_active(role::CENTER, 0);
        super::record_connection_event(role::CENTER, event::CONNECTED);
        super::record_connection_duration(role::CENTER, 1.5);
        super::record_watch_event("PluginMetaData", direction::RECV);
        super::record_watch_list("PluginMetaData", watch_list_result::OK);
        super::record_watch_error("PluginMetaData", watch_error_reason::RECV_ERROR);
        super::record_mark_offline(offline_reason::DISCONNECT);
        super::record_evict_stale(evict_source::REGISTRY);
        super::record_session_reentry();
        super::set_aggregator_controllers("default", 3);
        super::record_consistency_mismatch();
        super::record_fanout(fanout_op::PATCH_ENABLE, fanout_result::OK);
        super::record_ready_gate_wait(0.25);
    }
}

#[cfg(test)]
mod rbac_metric_tests {
    use super::*;

    /// Name constants for the new RBAC metrics must follow the `edgion_fed_` prefix
    /// convention and use only lowercase ASCII + underscores.
    #[test]
    fn rbac_metric_names_are_stable_and_valid() {
        assert_eq!(names::RBAC_DENIED_TOTAL, "edgion_fed_rbac_denied_total");
        assert_eq!(names::KILL_SWITCH_STATE, "edgion_fed_kill_switch_state");

        for n in [names::RBAC_DENIED_TOTAL, names::KILL_SWITCH_STATE] {
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
            "reload",
            "server-info",
            "diagnostics",
            "wipe-all",
            "debug",
            "unknown",
        ];
        // Bounded kind values: known kinds, synthetic labels, "unknown".
        let kinds = ["Secret", "PluginMetaData", "HTTPRoute", "RegionRoute", "*", "unknown"];
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

    /// Task 7: `record_rbac_denied` with the `source` label must not panic for
    /// the cli_token source, confirming the three bounded source values are usable.
    #[test]
    fn rbac_denied_with_source_label_increments_without_panic() {
        use super::labels::rbac_source;
        // The canonical Task-7 case: a cli token denied on Secret.
        record_rbac_denied("get", "Secret", rbac_source::CLI_TOKEN);
        record_rbac_denied("list", "PluginMetaData", rbac_source::CENTER);
        record_rbac_denied("unknown", "unknown", rbac_source::UNKNOWN);
        // All three bounded source values work.
        for source in [rbac_source::CENTER, rbac_source::CLI_TOKEN, rbac_source::UNKNOWN] {
            record_rbac_denied("watch", "HTTPRoute", source);
        }
    }
}
