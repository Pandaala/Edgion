use super::*;
use metrics_exporter_prometheus::PrometheusBuilder;

#[test]
fn etcd_counter_registration_is_bounded_and_cold_slots_are_lazy() {
    let recorder = PrometheusBuilder::new().build_recorder();
    metrics::with_local_recorder(&recorder, || {
        let owner = GatewayMetrics::etcd_operations("etcd-metrics/lazy");
        assert_eq!(
            owner
                .counters
                .iter()
                .flatten()
                .filter(|slot| slot.get().is_some())
                .count(),
            8
        );
        owner.record(EtcdOperation::Watch, EtcdOutcome::RecvLimit);
        owner.record(EtcdOperation::Watch, EtcdOutcome::RecvLimit);
        assert_eq!(
            owner
                .counters
                .iter()
                .flatten()
                .filter(|slot| slot.get().is_some())
                .count(),
            9
        );
        let rendered = GatewayMetrics::with_live_etcd_metrics(|| recorder.handle().render());
        let line = rendered
            .lines()
            .find(|line| {
                line.contains("link=\"etcd-metrics/lazy\"")
                    && line.contains("operation=\"watch\"")
                    && line.contains("outcome=\"recv_limit\"")
            })
            .unwrap();
        assert!(line.ends_with(" 2"));
    });
}

#[test]
fn etcd_owner_inventory_preserves_retired_owners_and_deduplicates_keys() {
    let first = GatewayMetrics::etcd_operations("etcd-metrics/overlap");
    let second = GatewayMetrics::etcd_operations("etcd-metrics/overlap");
    first.deactivate();
    {
        let owners = ETCD_OPERATION_METRICS.lock();
        assert_eq!(owners.get("etcd-metrics/overlap").unwrap().len(), 2);
    }
    let stats = GatewayMetrics::etcd_operation_metric_stats(usize::MAX);
    assert!(stats.count >= 2);
    assert_eq!(
        stats.keys.iter().filter(|key| *key == "etcd-metrics/overlap").count(),
        1
    );
    assert!(!stats.keys_truncated);
    assert!(GatewayMetrics::etcd_operation_metric_stats(0).keys_truncated);
    drop(first);
    drop(second);
    GatewayMetrics::etcd_operation_metric_stats(0);
    assert!(!ETCD_OPERATION_METRICS.lock().contains_key("etcd-metrics/overlap"));
}

#[test]
fn etcd_cached_completion_and_last_owner_drop_do_not_take_scrape_lock() {
    let owner = GatewayMetrics::etcd_operations("etcd-metrics/unblocked");
    let weak = Arc::downgrade(&owner);
    let locked = ETCD_OPERATION_METRICS.lock();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        owner.record(EtcdOperation::Kv, EtcdOutcome::Ok);
        drop(owner);
        tx.send(()).unwrap();
    });
    let done = rx.recv_timeout(std::time::Duration::from_secs(1)).is_ok();
    drop(locked);
    thread.join().unwrap();
    assert!(done);
    assert!(weak.upgrade().is_none());
}

#[test]
fn etcd_registration_uses_the_shared_lifetime_cardinality_guard() {
    use edgion_common::observe::cardinality_guard::{CardinalityGuard, CardinalityMode};
    for (mode, expected) in [(CardinalityMode::Enforce, 10), (CardinalityMode::Observe, 18)] {
        let recorder = PrometheusBuilder::new().build_recorder();
        let guard = CardinalityGuard::new(recorder, 10, mode);
        metrics::with_local_recorder(&guard, || {
            let first = GatewayMetrics::etcd_operations("etcd-metrics/budget-first");
            assert_eq!(guard.series_count(), 10); // Eight successes plus two guard self-metrics.
            drop(first);
            let second = GatewayMetrics::etcd_operations("etcd-metrics/budget-second");
            assert_eq!(guard.series_count(), expected); // Dropping an owner does not reset the lifetime budget.
            second.record(EtcdOperation::Kv, EtcdOutcome::Ok);
        });
    }
}

#[test]
fn scrape_keeps_active_cached_counters_attached_across_retired_generations() {
    let recorder = PrometheusBuilder::new()
        .idle_timeout(metrics_util::MetricKindMask::COUNTER, Some(std::time::Duration::ZERO))
        .build_recorder();
    metrics::with_local_recorder(&recorder, || {
        let old = GatewayMetrics::etcd_operations("etcd-metrics/idle");
        old.record(EtcdOperation::Kv, EtcdOutcome::Ok);
        let render = || GatewayMetrics::with_live_etcd_metrics(|| recorder.handle().render());
        for _ in 0..4 {
            assert!(render().contains("link=\"etcd-metrics/idle\""));
        }
        let replacement = GatewayMetrics::etcd_operations("etcd-metrics/idle");
        old.deactivate();
        replacement.record(EtcdOperation::Kv, EtcdOutcome::Ok);
        for _ in 0..4 {
            let output = render();
            let line = output
                .lines()
                .find(|line| line.contains("link=\"etcd-metrics/idle\"") && line.contains("operation=\"kv\""))
                .unwrap();
            assert!(line.ends_with(" 2"));
        }
        drop(old); // Retiring/dropping the old generation cannot clear the shared counter.
        replacement.record(EtcdOperation::Kv, EtcdOutcome::Ok);
        let output = render();
        let line = output
            .lines()
            .find(|line| line.contains("link=\"etcd-metrics/idle\"") && line.contains("operation=\"kv\""))
            .unwrap();
        assert!(line.ends_with(" 3"));
        replacement.deactivate();
        let _ = render();
        assert!(!render().contains("link=\"etcd-metrics/idle\""));
        // The exporter may evict retired series while residue still tracks a retained owner.
        assert!(GatewayMetrics::etcd_operation_metric_stats(usize::MAX)
            .keys
            .iter()
            .any(|key| key == "etcd-metrics/idle"));
    });
}
