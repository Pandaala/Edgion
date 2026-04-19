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
    /// Gauge: aggregated resource-key count in Center, by kind.
    pub const AGGREGATOR_KEYS_TOTAL: &str = "edgion_fed_aggregator_keys_total";
    /// Counter: consistency-check API detected cross-controller divergence.
    pub const CONSISTENCY_MISMATCH_TOTAL: &str = "edgion_fed_consistency_mismatch_total";
    /// Counter: fan-out patch operations' aggregate outcome.
    pub const FANOUT_TOTAL: &str = "edgion_fed_fanout_total";
    /// Gauge: last observed ready-gate wait in seconds (Controller side).
    pub const READY_GATE_WAIT_LAST: &str = "edgion_fed_ready_gate_wait_seconds_last";
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

/// Set the aggregated key count for a given resource kind.
#[inline]
pub fn set_aggregator_keys(kind: &str, count: u64) {
    gauge!(names::AGGREGATOR_KEYS_TOTAL, "kind" => kind.to_string()).set(count as f64);
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

#[cfg(test)]
mod tests {
    use super::labels::*;

    #[test]
    fn role_labels_are_stable() {
        assert_eq!(role::CENTER, "center");
        assert_eq!(role::CONTROLLER, "controller");
    }

    #[test]
    fn event_labels_are_stable() {
        assert_eq!(event::CONNECTED, "connected");
        assert_eq!(event::DISCONNECTED, "disconnected");
        assert_eq!(event::RELOAD, "reload");
    }

    #[test]
    fn direction_labels_are_stable() {
        assert_eq!(direction::SENT, "sent");
        assert_eq!(direction::RECV, "recv");
    }

    #[test]
    fn watch_list_result_labels_are_stable() {
        assert_eq!(watch_list_result::OK, "ok");
        assert_eq!(watch_list_result::VERSION_TOO_OLD, "version_too_old");
        assert_eq!(watch_list_result::PARSE_ERROR, "parse_error");
        assert_eq!(watch_list_result::READY_WAIT, "ready_wait");
    }

    #[test]
    fn watch_error_reason_labels_are_stable() {
        assert_eq!(watch_error_reason::PARSE_ERROR, "parse_error");
        assert_eq!(watch_error_reason::TIMEOUT, "timeout");
        assert_eq!(watch_error_reason::RECV_ERROR, "recv_error");
    }

    #[test]
    fn offline_reason_labels_are_stable() {
        assert_eq!(offline_reason::HEARTBEAT, "heartbeat");
        assert_eq!(offline_reason::DISCONNECT, "disconnect");
        assert_eq!(offline_reason::RELOAD, "reload");
    }

    #[test]
    fn evict_source_labels_are_stable() {
        assert_eq!(evict_source::REGISTRY, "registry");
        assert_eq!(evict_source::AGGREGATOR, "aggregator");
    }

    #[test]
    fn fanout_labels_are_stable() {
        assert_eq!(fanout_op::PATCH_ENABLE, "patch_enable");
        assert_eq!(fanout_op::PATCH_PROFILE, "patch_profile");
        assert_eq!(fanout_result::OK, "ok");
        assert_eq!(fanout_result::PARTIAL, "partial");
        assert_eq!(fanout_result::FAIL, "fail");
    }

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
            AGGREGATOR_KEYS_TOTAL,
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
        super::set_aggregator_keys("PluginMetaData", 42);
        super::record_consistency_mismatch();
        super::record_fanout(fanout_op::PATCH_ENABLE, fanout_result::OK);
        super::record_ready_gate_wait(0.25);
    }
}
