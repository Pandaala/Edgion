use super::*;
use metrics_util::debugging::DebuggingRecorder;

#[test]
fn record_link_sys_health_sets_gauge_and_does_not_panic() {
    // Smoke: gauge set path is callable for both states.
    record_link_sys_health("default/kafka-main", true);
    record_link_sys_health("default/kafka-main", false);
}

#[test]
fn record_link_sys_health_flip_emits_direction_label() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    metrics::with_local_recorder(&recorder, || {
        record_link_sys_health_flip("default/kafka-main", true);
        record_link_sys_health_flip("default/kafka-main", false);
    });
    let snap = snapshotter.snapshot().into_vec();
    let key = snap
        .iter()
        .find(|(k, _, _, _)| k.key().name() == names::LINK_SYS_HEALTH_FLIPS_TOTAL)
        .expect("flip counter registered");
    let labels: Vec<(&str, &str)> = key.0.key().labels().map(|l| (l.key(), l.value())).collect();
    assert!(labels.contains(&("link", "default/kafka-main")));
    assert!(labels.contains(&("direction", "recovered")));
    let degraded = snap.iter().any(|(k, _, _, _)| {
        k.key().name() == names::LINK_SYS_HEALTH_FLIPS_TOTAL
            && k.key()
                .labels()
                .any(|l| l.key() == "direction" && l.value() == "degraded")
    });
    assert!(degraded, "degraded flip counter must be registered");
}
