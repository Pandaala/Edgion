//! Controller-side federation metrics.
//!
//! Emits Prometheus counters/gauges for the federation plane from the
//! Controller's point of view: connection lifecycle, watch events it serves,
//! ready-gate wait, admin-API RBAC denials, kill-switch state, and whether the
//! federation client is actually running in this process. See
//! `docs/superpowers/specs/2026-04-19-fix-24-fed-metrics-baseline-design.md`
//! for the full design.
//!
//! **Label value discipline:** every label value emitted here must come
//! from a bounded enum (the `labels` submodule) or from a well-known
//! resource-kind short name. Do NOT pass raw user input, `controller_id`,
//! hostnames, UUIDs, or free-form paths as label values — those explode
//! Prometheus cardinality.

use super::metrics_facade::{fixed_counter, fixed_gauge};

/// Metric name constants. Kept here as a single source of truth so tests
/// and docs can reference the exact strings exposed on `/metrics`.
pub mod names {
    /// Gauge: currently active federation connections (from the emitting
    /// process's point of view). On Controller this is 0 or 1.
    pub const CONNECTIONS_ACTIVE: &str = "edgion_fed_connections_active";
    /// Counter: federation session lifecycle events.
    pub const CONNECTION_EVENTS_TOTAL: &str = "edgion_fed_connection_events_total";
    /// Gauge: last observed session duration in seconds (recorded at
    /// disconnect time).
    pub const CONNECTION_DURATION_LAST: &str = "edgion_fed_connection_duration_seconds_last";
    /// Counter: watch events flowing across the federation link.
    pub const WATCH_EVENTS_TOTAL: &str = "edgion_fed_watch_events_total";
    /// Gauge: last observed ready-gate wait in seconds (Controller side).
    pub const READY_GATE_WAIT_LAST: &str = "edgion_fed_ready_gate_wait_seconds_last";
    /// Counter: RBAC denials on the Controller admin-API federation path.
    /// Labels: `verb` (closed set from `Verb::as_str()` + "unknown"),
    /// `kind` (known ResourceKind, "*", "RegionRoute", or "unknown" — never raw input),
    /// `source` ("center" | "cli_token" | "unknown" — identifies the auth path).
    pub const RBAC_DENIED_TOTAL: &str = "edgion_fed_rbac_denied_total";
    /// Gauge: federation kill-switch state — the OPERATOR'S intent
    /// (`center.enabled`) only. 1.0 = enabled, 0.0 = disabled.
    pub const KILL_SWITCH_STATE: &str = "edgion_fed_kill_switch_state";
    /// Gauge: whether the federation client is actually running in this
    /// process. 1.0 = running, 0.0 = not running.
    ///
    /// This is deliberately separate from `KILL_SWITCH_STATE`: the federation
    /// session is a process-wide singleton keyed by `controller_id`, so only
    /// the leader replica runs the client. On a follower the kill switch is
    /// still 1 (the operator never touched it) while this gauge is 0 — folding
    /// the two together would make a dashboard report a kill switch that
    /// nobody engaged.
    pub const CLIENT_RUNNING: &str = "edgion_fed_client_running";
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
    fixed_gauge!(names::CONNECTIONS_ACTIVE, "role" => role).set(count as f64);
}

/// Record a connection lifecycle event.
#[inline]
pub fn record_connection_event(role: &'static str, event: &'static str) {
    fixed_counter!(names::CONNECTION_EVENTS_TOTAL, "role" => role, "event" => event).increment(1);
}

/// Record the most recent session duration (observed at disconnect).
#[inline]
pub fn record_connection_duration(role: &'static str, seconds: f64) {
    fixed_gauge!(names::CONNECTION_DURATION_LAST, "role" => role).set(seconds);
}

// ---------- Watch plane ----------

/// Record a watch event crossing the federation link.
///
/// `kind` must come from a bounded set of resource kinds (e.g.
/// "EdgionConfigData"). Never pass raw user input.
#[inline]
pub fn record_watch_event(kind: &str, direction: &'static str) {
    fixed_counter!(names::WATCH_EVENTS_TOTAL, "kind" => kind.to_string(), "direction" => direction).increment(1);
}

// ---------- Ready gate (Controller) ----------

#[inline]
pub fn record_ready_gate_wait(seconds: f64) {
    fixed_gauge!(names::READY_GATE_WAIT_LAST).set(seconds);
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
    fixed_counter!(names::RBAC_DENIED_TOTAL,
        "verb" => verb.to_string(),
        "kind" => kind.to_string(),
        "source" => source.to_string()
    )
    .increment(1);
}

/// Set the federation kill-switch state gauge.
///
/// Reports the OPERATOR'S intent only (`center.enabled`), never the derived
/// running state. Call this on every supervisor `apply` so the current intent
/// is always visible in Prometheus without querying event history.
/// Value: `1.0` when federation is enabled in config, `0.0` when the operator
/// disabled it (kill-switch active).
#[inline]
pub fn record_kill_switch_state(enabled: bool) {
    fixed_gauge!(names::KILL_SWITCH_STATE).set(if enabled { 1.0 } else { 0.0 });
}

/// Set the federation client running gauge (see `names::CLIENT_RUNNING`).
///
/// `kill_switch_state == 1 && client_running == 0` is the normal, healthy
/// steady state of a non-leader Controller replica: federation is enabled but
/// only the leader may hold the session.
#[inline]
pub fn record_client_running(running: bool) {
    fixed_gauge!(names::CLIENT_RUNNING).set(if running { 1.0 } else { 0.0 });
}

#[cfg(test)]
#[path = "../../tests/unit/observe/fed_metrics_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/unit/observe/fed_metrics_test/rbac_metric_tests.rs"]
mod rbac_metric_tests;
