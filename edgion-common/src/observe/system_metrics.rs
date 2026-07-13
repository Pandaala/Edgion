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
//! them). A few reasons instead track a *transient* runtime condition (a TLS
//! handshake, a cert reload, a rotation tick) and are explicitly cleared via
//! [`clear_system_error`] on the next matching success — see each reason's
//! doc comment for whether it latches or clears.
//!
//! **Label discipline**: `component` and `reason` must both come from the
//! bounded constants in `components` and `reasons` below — never pass raw
//! config values or free-form strings (Prometheus cardinality risk).

use metrics::gauge;

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
    /// conf_sync `conf_sync_security` was omitted entirely — the channel is
    /// running in PLAINTEXT while it carries resolved Secrets to the data plane.
    /// Distinct from `skip_tls` (certs present but bypassed) and from federation's
    /// `no_tls_configured` (which means *refused to start*, fail-close); this one
    /// means a secret-carrying channel is *running* in plaintext.
    pub const PLAINTEXT_DEFAULT: &str = "plaintext_default";
    /// Gateway integration testing mode is active (`--integration-testing-mode`
    /// plus the `EDGION_ALLOW_INTEGRATION_TESTING_MODE` env override). Activates
    /// test-only admin routes, in-memory access-log storage, per-request metric
    /// labels, and loopback-upstream allowance. Production-unsafe; surfaced here
    /// so accidental enablement is caught by monitoring, not just a startup log.
    pub const INTEGRATION_TESTING_MODE: &str = "integration_testing_mode";
    /// A per-connection conf_sync gRPC server TLS handshake failed or timed out
    /// (bad client cert, untrusted CA, protocol mismatch, 10s handshake timeout).
    /// The connection is dropped without affecting the listener or other sessions;
    /// this reason surfaces a sustained rate of such failures to monitoring.
    /// Set on failure, cleared on the next successful handshake (alert on a
    /// sustained `== 1`, not a single blip).
    pub const TLS_HANDSHAKE_FAILED: &str = "tls_handshake_failed";
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
    gauge!(
        names::SYSTEM_ERROR_STATE,
        "component" => component,
        "reason" => reason,
    )
    .set(1.0);
}

/// Clear a transient system-level error condition.
///
/// Sets `edgion_system_error_state{component, reason} = 0`. Use this only for
/// reasons whose doc comment says they clear on success (e.g.
/// `TLS_HANDSHAKE_FAILED`, `CERT_RELOAD_INVALID`, `CERT_ROTATION_FAILED`) —
/// startup-config reasons are meant to latch and must not be cleared. Calling
/// this for a `(component, reason)` pair that was never marked is harmless: it
/// just sets a fresh gauge to 0.
///
/// `component` should come from [`components`] and `reason` from [`reasons`].
pub fn clear_system_error(component: &'static str, reason: &'static str) {
    gauge!(
        names::SYSTEM_ERROR_STATE,
        "component" => component,
        "reason" => reason,
    )
    .set(0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;

    #[test]
    fn mark_system_error_emits_gauge_with_labels() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            mark_system_error(components::CONF_SYNC_CLIENT, reasons::SKIP_TLS);

            let rendered = handle.render();
            assert!(
                rendered.contains("edgion_system_error_state"),
                "metric name missing, got:\n{rendered}"
            );
            assert!(
                rendered.contains("component=\"conf_sync_client\""),
                "component label missing, got:\n{rendered}"
            );
            assert!(
                rendered.contains("reason=\"skip_tls\""),
                "reason label missing, got:\n{rendered}"
            );
            // Gauge value should be 1 for the active error state.
            assert!(
                rendered.lines().any(|l| l.contains("edgion_system_error_state{")
                    && l.contains("component=\"conf_sync_client\"")
                    && l.contains("reason=\"skip_tls\"")
                    && l.trim_end().ends_with(" 1")),
                "expected gauge value of 1, got:\n{rendered}"
            );
        });
    }

    #[test]
    fn plaintext_default_reason_renders_with_labels() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            mark_system_error(components::CONF_SYNC_CLIENT, reasons::PLAINTEXT_DEFAULT);

            let rendered = handle.render();
            assert!(
                rendered.lines().any(|l| l.contains("edgion_system_error_state{")
                    && l.contains("component=\"conf_sync_client\"")
                    && l.contains("reason=\"plaintext_default\"")
                    && l.trim_end().ends_with(" 1")),
                "expected plaintext_default gauge = 1, got:\n{rendered}"
            );
        });
    }

    #[test]
    fn multiple_components_coexist_on_same_reason() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            mark_system_error(components::CONF_SYNC_CLIENT, reasons::SKIP_TLS);
            mark_system_error(components::CONF_SYNC_SERVER, reasons::SKIP_TLS);

            let rendered = handle.render();
            assert!(rendered.contains("component=\"conf_sync_client\""));
            assert!(rendered.contains("component=\"conf_sync_server\""));
        });
    }

    #[test]
    fn admin_insecure_exposure_emits_gauge_with_bounded_labels() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            mark_system_error(components::ADMIN_API, reasons::INSECURE_ADMIN_EXPOSURE);

            let rendered = handle.render();
            assert!(
                rendered.lines().any(|l| l.contains("edgion_system_error_state{")
                    && l.contains("component=\"admin_api\"")
                    && l.contains("reason=\"insecure_admin_exposure\"")
                    && l.trim_end().ends_with(" 1")),
                "expected admin_api/insecure_admin_exposure gauge = 1, got:\n{rendered}"
            );
        });
    }

    #[test]
    fn integration_testing_mode_emits_gauge_with_bounded_labels() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            mark_system_error(components::GATEWAY_RUNTIME, reasons::INTEGRATION_TESTING_MODE);

            let rendered = handle.render();
            assert!(
                rendered.lines().any(|l| l.contains("edgion_system_error_state{")
                    && l.contains("component=\"gateway_runtime\"")
                    && l.contains("reason=\"integration_testing_mode\"")
                    && l.trim_end().ends_with(" 1")),
                "expected gateway_runtime/integration_testing_mode gauge = 1, got:\n{rendered}"
            );
        });
    }

    #[test]
    fn clear_system_error_resets_gauge_to_zero() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            mark_system_error(components::CONF_SYNC_SERVER, reasons::CERT_ROTATION_FAILED);
            clear_system_error(components::CONF_SYNC_SERVER, reasons::CERT_ROTATION_FAILED);

            let rendered = handle.render();
            assert!(
                rendered.lines().any(|l| l.contains("edgion_system_error_state{")
                    && l.contains("component=\"conf_sync_server\"")
                    && l.contains("reason=\"cert_rotation_failed\"")
                    && l.trim_end().ends_with(" 0")),
                "expected cert_rotation_failed gauge reset to 0, got:\n{rendered}"
            );
        });
    }

    #[test]
    fn clear_system_error_on_never_marked_pair_is_harmless() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            // No prior mark_system_error call for this pair.
            clear_system_error(components::CONF_SYNC_CLIENT, reasons::CERT_RELOAD_INVALID);

            let rendered = handle.render();
            assert!(
                rendered.lines().any(|l| l.contains("edgion_system_error_state{")
                    && l.contains("component=\"conf_sync_client\"")
                    && l.contains("reason=\"cert_reload_invalid\"")
                    && l.trim_end().ends_with(" 0")),
                "expected cert_reload_invalid gauge = 0, got:\n{rendered}"
            );
        });
    }
}
