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

#[test]
fn redis_connection_gauges_carry_live_and_configured_states() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    metrics::with_local_recorder(&recorder, || {
        record_link_sys_redis_connections("default/redis-main", 5, 8);
    });
    let snap = snapshotter.snapshot().into_vec();
    let value = |state: &str| {
        snap.iter()
            .find(|(k, _, _, _)| {
                k.key().name() == names::LINK_SYS_REDIS_CONNECTIONS
                    && k.key()
                        .labels()
                        .any(|l| l.key() == "link" && l.value() == "default/redis-main")
                    && k.key().labels().any(|l| l.key() == "state" && l.value() == state)
            })
            .map(|(_, _, _, value)| format!("{value:?}"))
            .unwrap_or_else(|| panic!("{state} gauge registered"))
    };
    assert!(value("live").contains("5.0"), "{}", value("live"));
    assert!(value("configured").contains("8.0"), "{}", value("configured"));
}

#[test]
fn redis_topology_events_use_the_fixed_event_label_set() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let events = [
        RedisTopologyEvent::RecoveryFailed,
        RedisTopologyEvent::Recovered,
        RedisTopologyEvent::PrimaryChanged,
    ];
    metrics::with_local_recorder(&recorder, || {
        for event in events {
            record_link_sys_redis_topology_event("default/redis-main", event);
        }
    });
    let snap = snapshotter.snapshot().into_vec();
    for event in ["recovery_failed", "recovered", "primary_changed"] {
        assert!(
            snap.iter().any(|(k, _, _, _)| {
                k.key().name() == names::LINK_SYS_REDIS_TOPOLOGY_EVENTS_TOTAL
                    && k.key().labels().any(|l| l.key() == "event" && l.value() == event)
                    && k.key()
                        .labels()
                        .any(|l| l.key() == "link" && l.value() == "default/redis-main")
            }),
            "{event} counter registered"
        );
    }
}
