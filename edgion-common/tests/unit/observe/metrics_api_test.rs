#[test]
fn guard_self_metrics_registered_when_enabled() {
    use crate::observe::cardinality_guard::{CardinalityGuard, CardinalityMode};
    use metrics_exporter_prometheus::PrometheusBuilder;
    // Construct a guard directly (do not touch the process-global recorder,
    // which can only be set once): a fresh recorder wrapped at cap>0 renders
    // the two self-metric names.
    let rec = PrometheusBuilder::new().build_recorder();
    let h = rec.handle();
    let _g = CardinalityGuard::new(rec, 10_000, CardinalityMode::Enforce);
    let out = h.render();
    assert!(out.contains("edgion_metrics_cardinality_limit_active"));
}

#[test]
fn arbitrary_service_role_cannot_install_a_production_recorder() {
    use crate::observe::{
        cardinality_guard::CardinalityMode,
        metrics_api::{install_global_recorder, recorder_installed, RecorderConfig},
    };
    let before = recorder_installed();
    for service in ["tenant-name", "resource-123", "test", "bench", "edgion-cli"] {
        assert!(install_global_recorder(RecorderConfig {
            service,
            idle_timeout_secs: 600,
            histogram_buckets: &[],
            cardinality_cap: 10000,
            cardinality_mode: CardinalityMode::Enforce
        })
        .is_err());
        assert_eq!(recorder_installed(), before);
    }
}
