use super::*;
use metrics::{Key, Metadata, Recorder};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle, PrometheusRecorder};

const MD: Metadata<'static> = Metadata::new("test", metrics::Level::INFO, None);

fn guarded(cap: usize, mode: CardinalityMode) -> (CardinalityGuard<PrometheusRecorder>, PrometheusHandle) {
    let rec = PrometheusBuilder::new().build_recorder();
    let handle = rec.handle();
    (CardinalityGuard::new(rec, cap, mode), handle)
}

#[test]
fn allows_under_cap() {
    // cap 4 = 2 self slots + 2 user metrics
    let (g, h) = guarded(4, CardinalityMode::Enforce);
    g.register_counter(&Key::from_name("edgion_u_a_total"), &MD)
        .increment(1);
    g.register_counter(&Key::from_name("edgion_u_b_total"), &MD)
        .increment(1);
    let out = h.render();
    assert!(out.contains("edgion_u_a_total"));
    assert!(out.contains("edgion_u_b_total"));
}

#[test]
fn rejects_new_over_cap_enforce() {
    let (g, h) = guarded(4, CardinalityMode::Enforce);
    g.register_counter(&Key::from_name("edgion_u_a_total"), &MD)
        .increment(1);
    g.register_counter(&Key::from_name("edgion_u_b_total"), &MD)
        .increment(1);
    g.register_counter(&Key::from_name("edgion_u_c_total"), &MD)
        .increment(1);
    let out = h.render();
    assert!(!out.contains("edgion_u_c_total"));
    assert!(out.contains("edgion_metrics_series_rejected_total"));
    assert!(out.contains("edgion_metrics_cardinality_limit_active 1"));
}

#[test]
fn known_key_repeats_do_not_grow() {
    let (g, _h) = guarded(4, CardinalityMode::Enforce);
    let k = Key::from_name("edgion_u_a_total");
    let _ = g.register_counter(&k, &MD);
    let before = g.series_count();
    let _ = g.register_counter(&k, &MD);
    assert_eq!(before, g.series_count());
}

#[test]
fn observe_mode_registers_but_flags() {
    let (g, h) = guarded(4, CardinalityMode::Observe);
    g.register_counter(&Key::from_name("edgion_u_a_total"), &MD)
        .increment(1);
    g.register_counter(&Key::from_name("edgion_u_b_total"), &MD)
        .increment(1);
    g.register_counter(&Key::from_name("edgion_u_c_total"), &MD)
        .increment(1);
    let out = h.render();
    assert!(out.contains("edgion_u_c_total")); // NOT dropped in observe
    assert!(out.contains("edgion_metrics_cardinality_limit_active 1"));
}

// Unit-only access to the actual soft-admission permission/commit interval.
// A sequentially denied key cannot recover: the ledger never refunds capacity.
impl Admission {
    pub(crate) fn test_cold_permission(&self, key: &Key) -> bool {
        self.permitted(key)
    }
    pub(crate) fn test_commit_prior_permission(&self, key: &Key, permitted: bool) {
        assert!(permitted, "commit requires a permission granted before saturation");
        // This is the identity-entry step used by dynamic_slot. The public
        // facade subsequently creates the real slot and attaches contributions.
        self.seen.entry(key.clone()).or_insert(None);
    }
}
