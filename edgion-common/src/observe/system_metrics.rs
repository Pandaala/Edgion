//! System-level error state metrics.
//!
//! A single registry for "system-level" degraded states across Edgion
//! binaries (Gateway, Controller). Emits a Prometheus gauge that
//! stays at `1` while a degraded mode is active, so operators can wire
//! an Alertmanager rule like:
//!
//! ```text
//! edgion_system_error_state > 0 for 5m
//! ```
//!
//! This is *observability* only — the metric does not change the system's
//! runtime behavior. It exists so a deliberately-insecure or degraded
//! operational mode (e.g., `skip_tls` bypass, future similar toggles)
//! surfaces in monitoring even when operators stop reading startup logs.
//!
//! Most reasons are startup-time configuration decisions and latch for the
//! life of the process (by design — restarting is the only way to change
//! them). A few reasons instead track a *transient* runtime condition (a link
//! going down, a cert reload, a rotation tick) and are explicitly cleared via
//! [`clear_system_error`] on the next matching success — see each reason's
//! doc comment for whether it latches or clears.
//!
//! **A latching reason needs a single owner — and the owner must be the process
//! that is actually broken.** This gauge is keyed only by `(component, reason)`,
//! so a condition observed once *per peer* (many gateways against one controller)
//! must NOT be a reason here: the healthy peers' successes would clear the broken
//! peer's mark and the alert could never fire. Do not reach for a `peer` label to
//! fix that — on a pre-auth path (a TLS handshake *is* the authentication) any
//! remote caller could then mint unbounded series at will. Prefer instead: let the
//! broken peer raise the mark on *its own* registry (as the gateway does with
//! `CONFSYNC_DISCONNECTED`), and leave the per-peer detail in a rate-limited log,
//! which carries the peer identity a bounded metric never could.
//!
//! **Label discipline**: `component` and `reason` must both come from the
//! bounded constants in `components` and `reasons` below — never pass raw
//! config values or free-form strings (Prometheus cardinality risk).

use super::metrics_facade::fixed_gauge;

/// Metric name constants.
pub mod names {
    /// Gauge = 1 while the `(component, reason)` pair's error condition
    /// is active; stays at 1 until the process restarts (these are
    /// startup-time config decisions, not transient runtime failures).
    pub const SYSTEM_ERROR_STATE: &str = "edgion_system_error_state";
}

/// Component label values — which subsystem is reporting the error.
///
/// Keep this list small and closed (bounded cardinality).
pub mod components {
    /// Gateway-side conf_sync gRPC client (Gateway → Controller).
    pub const CONF_SYNC_CLIENT: &str = "conf_sync_client";
    /// Controller-side conf_sync gRPC server.
    pub const CONF_SYNC_SERVER: &str = "conf_sync_server";
    /// Controller-side federation gRPC client (Controller → Center).
    pub const FED_SYNC_CLIENT: &str = "fed_sync_client";
    /// Controller-side Admin HTTP API server.
    pub const ADMIN_API: &str = "admin_api";
    /// Gateway data-plane runtime (process-wide mode toggles).
    pub const GATEWAY_RUNTIME: &str = "gateway_runtime";
}

/// Reason label values — what kind of system-level error condition.
///
/// Keep this list small and closed (bounded cardinality).
pub mod reasons {
    /// TLS was configured but `skip_tls=true` is in effect — the channel
    /// is running in plaintext despite cert paths being present.
    /// Emergency bypass; must not stay enabled in production.
    pub const SKIP_TLS: &str = "skip_tls";
    /// Federation requires mTLS but no TLS block was configured; the channel
    /// refused to start (fail-close). Distinct from a *running* plaintext
    /// channel — federation never runs in plaintext.
    pub const NO_TLS_CONFIGURED: &str = "no_tls_configured";
    /// Controller's own client cert SPIFFE SAN does not match its configured
    /// cluster/name; federation client was not started.
    pub const PEER_IDENTITY_SELF_CHECK: &str = "peer_identity_self_check";
    /// Admin API is bound to a non-loopback address with no IP allowlist and no
    /// TLS — the privileged admin plane is reachable on every routable interface
    /// in plaintext. A deliberate-exposure flag for monitoring, not a blocker.
    pub const INSECURE_ADMIN_EXPOSURE: &str = "insecure_admin_exposure";
    /// Gateway integration testing mode is active (`--integration-testing-mode`
    /// plus the `EDGION_ALLOW_INTEGRATION_TESTING_MODE` env override). Activates
    /// test-only admin routes, in-memory access-log storage, per-request metric
    /// labels, and loopback-upstream allowance. Production-unsafe; surfaced here
    /// so accidental enablement is caught by monitoring, not just a startup log.
    pub const INTEGRATION_TESTING_MODE: &str = "integration_testing_mode";
    /// The gateway's conf_sync `WatchServerMeta` stream to the controller is down: it
    /// could not be established, or an established stream ended/errored and the client
    /// is backing off to reconnect. The gateway keeps serving traffic from its
    /// last-known config while this is set — that is deliberate (a control-plane blip
    /// must not cost data-plane availability), which is why the condition needs a
    /// signal at all: a gateway that can *never* reconnect (expired cert, dead
    /// controller) otherwise serves ever-staler config indefinitely, and its `/ready`
    /// probe keeps saying yes (readiness latches at boot, by design).
    ///
    /// Set when a reconnect cycle fails, cleared once the stream is re-established.
    /// An intentional reconnect (server switch, client TLS hot reload) does NOT set
    /// it — those skip backoff and reconnect on the next channel immediately.
    ///
    /// **Scope — do not read this as "the gateway is in sync".** It tracks the
    /// `WatchServerMeta` stream only. The per-resource-kind list/watch loops
    /// (`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs`) are independent
    /// tasks with their own backoff and do not touch this gauge, so an
    /// application-level failure confined to them (a list response the gateway cannot
    /// parse, a persistent server-id mismatch) leaves this at 0 while those caches go
    /// stale. Transport-level failures do surface here, since every kind's watch shares
    /// the one tonic `Channel` this stream rides. Their absence of a metric is
    /// pre-existing and deliberate (`tracing::error!` + status is the control-plane
    /// default); this gauge does not claim to cover them.
    ///
    /// **Alert on a sustained `== 1`** (e.g. `for: 10m`), never on a single blip:
    /// a controller rollout legitimately flips this to 1 for a few seconds. Naturally
    /// per-pod — each gateway has its own registry and exactly one controller peer.
    pub const CONFSYNC_DISCONNECTED: &str = "confsync_disconnected";
    /// The conf_sync auto-CA `rotation` schedule/TTL combination was accepted but
    /// flagged as a suspect combo by `cert_util::validate_schedule_controller`
    /// (e.g. a rotation cadence that leaves little headroom before leaf expiry).
    pub const CERT_ROTATION_CONFIG_SUSPECT: &str = "cert_rotation_config_suspect";
    /// The controller's file-mode cert watcher detected a change on disk but the
    /// re-read cert/key/CA failed `ServerIdentity::from_pems` validation (bad PEM,
    /// mismatched key, untrusted chain, ...). The previous identity is kept in
    /// place (validate-before-swap); this reason surfaces a rejected reload so a
    /// bad Secret update doesn't silently freeze the server on stale material.
    /// Set on a rejected reload, cleared on the next successful reload (alert on
    /// a sustained `== 1`, not a single blip).
    pub const CERT_RELOAD_INVALID: &str = "cert_reload_invalid";
    /// The conf_sync auto-CA rotation reconciler's periodic check-and-rotate tick
    /// failed (Secret GET/apply error, signing failure, ...). The reconciler backs
    /// off exponentially and retries on the next tick; the previously served
    /// identity and leaf Secret stay in place. Set on a failed tick, cleared on
    /// the next successful tick (alert on a sustained `== 1`, not a single blip;
    /// a stalled rotation eventually reaches the emergency window).
    pub const CERT_ROTATION_FAILED: &str = "cert_rotation_failed";
    /// The per-replica conf_sync server-identity reconciler's tick failed (CA Secret
    /// GET error, signing failure, identity build failure, ...). That loop runs on
    /// EVERY controller replica (leader or not) and keeps this replica's own serving
    /// leaf — and the client-CA verifier that authenticates gateways — in sync with
    /// the current root. While it is failing, this replica's serving identity may be
    /// STALE against the current root: under `ha_mode: all-serve` that means it may
    /// still accept a client leaf signed by a revoked root (an emergency CA re-key
    /// would not actually revoke on this pod). Set on a failed tick, cleared on the
    /// next successful one.
    ///
    /// **Deliberately distinct from [`CERT_ROTATION_FAILED`]**: that reason belongs to
    /// the leader-only rotation reconciler, which is a WRITE path (Secret applies,
    /// signing) and backs off exponentially. The two loops share a component
    /// (`conf_sync_server`) and both run on the leader, so if they shared a reason the
    /// read-only identity loop — which succeeds almost always, even with nothing to do
    /// — would clear the rotation loop's mark every 60s and silence the page. Never let
    /// a loop clear a mark it did not set.
    pub const CERT_IDENTITY_SYNC_FAILED: &str = "cert_identity_sync_failed";
    /// The filesystem watcher backing cert hot reload could not be established —
    /// either the `notify` watcher itself failed to construct, or one of the cert
    /// parent directories could not be watched. Cert hot reload has degraded to
    /// the periodic re-stat backstop only: a rotated cert is still picked up, but
    /// after up to one re-stat interval (5 minutes) instead of ~2 seconds.
    /// **Latches** — the watch is never retried, so this stays at 1 until the
    /// process restarts. Do NOT clear it via [`clear_system_error`].
    pub const CERT_WATCH_FAILED: &str = "cert_watch_failed";
}

/// Mark a system-level error condition as active.
///
/// Sets `edgion_system_error_state{component, reason} = 1`. Most reasons are
/// startup-time configuration decisions and stay at 1 for the lifetime of the
/// process; reasons whose doc comment says they clear on success are reset via
/// [`clear_system_error`] by their success path.
///
/// `component` should come from [`components`] and `reason` from
/// [`reasons`]. Callers that pass ad-hoc strings will pollute Prometheus
/// cardinality and should be audited.
pub fn mark_system_error(component: &'static str, reason: &'static str) {
    fixed_gauge!(names::SYSTEM_ERROR_STATE,
        "component" => component,
        "reason" => reason,
    )
    .set(1.0);
}

/// Clear a transient system-level error condition.
///
/// Sets `edgion_system_error_state{component, reason} = 0`. Use this only for
/// reasons whose doc comment says they clear on success (e.g.
/// `CONFSYNC_DISCONNECTED`, `CERT_RELOAD_INVALID`, `CERT_ROTATION_FAILED`) —
/// startup-config reasons are meant to latch and must not be cleared. Calling
/// this for a `(component, reason)` pair that was never marked is harmless: it
/// just sets a fresh gauge to 0.
///
/// `component` should come from [`components`] and `reason` from [`reasons`].
pub fn clear_system_error(component: &'static str, reason: &'static str) {
    fixed_gauge!(names::SYSTEM_ERROR_STATE,
        "component" => component,
        "reason" => reason,
    )
    .set(0.0);
}

#[cfg(test)]
#[path = "../../tests/unit/observe/system_metrics_test.rs"]
mod tests;
