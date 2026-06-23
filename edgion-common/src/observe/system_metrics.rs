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
}

/// Mark a system-level error condition as active.
///
/// Sets `edgion_system_error_state{component, reason} = 1`. The gauge
/// stays at 1 for the lifetime of the process — these are startup-time
/// configuration decisions, not transient errors.
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
}
