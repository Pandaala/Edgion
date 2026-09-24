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
