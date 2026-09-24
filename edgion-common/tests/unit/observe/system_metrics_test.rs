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

/// Regression guard for the alert-suppression bug: the per-replica identity
/// reconciler must NOT share `CERT_ROTATION_FAILED` with the leader-only rotation
/// reconciler. Both run on the leader under the same component, and the read-only
/// identity loop succeeds almost always — sharing the reason would drive the
/// rotation page's gauge back to 0 every tick. Never let a loop clear a mark it
/// did not set.
#[test]
fn identity_sync_failure_does_not_clear_the_rotation_mark() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    assert_ne!(reasons::CERT_IDENTITY_SYNC_FAILED, reasons::CERT_ROTATION_FAILED);

    metrics::with_local_recorder(&recorder, || {
        // The rotation reconciler is failing (write path: RBAC denied, apply rejected).
        mark_system_error(components::CONF_SYNC_SERVER, reasons::CERT_ROTATION_FAILED);
        // The read-only identity reconciler ticks successfully on the same replica.
        clear_system_error(components::CONF_SYNC_SERVER, reasons::CERT_IDENTITY_SYNC_FAILED);

        let rendered = handle.render();
        assert!(
            rendered.lines().any(|l| l.contains("edgion_system_error_state{")
                && l.contains("component=\"conf_sync_server\"")
                && l.contains("reason=\"cert_rotation_failed\"")
                && l.trim_end().ends_with(" 1")),
            "the rotation page must stay armed at 1, got:\n{rendered}"
        );
        assert!(
            rendered.lines().any(|l| l.contains("edgion_system_error_state{")
                && l.contains("component=\"conf_sync_server\"")
                && l.contains("reason=\"cert_identity_sync_failed\"")
                && l.trim_end().ends_with(" 0")),
            "expected cert_identity_sync_failed gauge = 0, got:\n{rendered}"
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
