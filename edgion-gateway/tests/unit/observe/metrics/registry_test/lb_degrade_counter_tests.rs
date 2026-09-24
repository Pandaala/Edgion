use super::*;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::Barrier;

#[test]
fn degrade_counter_last_owner_does_not_wait_for_scrape_before_weak_upgrade() {
    let counter = GatewayMetrics::lb_degrade_selections("pin/unpinned-drop");
    let weak = Arc::downgrade(&counter);
    let locked = LB_DEGRADE_COUNTERS.lock();
    let (completed, completion) = std::sync::mpsc::channel();
    let release = std::thread::spawn(move || {
        drop(counter);
        completed.send(()).unwrap();
    });
    let completed_while_locked = completion.recv_timeout(std::time::Duration::from_secs(1)).is_ok();
    drop(locked);
    release.join().unwrap();
    assert!(
        completed_while_locked,
        "last-owner release must not wait for metric rendering"
    );
    assert!(weak.upgrade().is_none());
    GatewayMetrics::lb_degrade_metric_stats(0);
    assert!(!lock_degrade_counters().contains_key("pin/unpinned-drop"));
}

#[test]
fn degrade_counter_scrape_pins_the_last_owner_without_blocking_event_writes() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let counter = metrics::with_local_recorder(&recorder, || GatewayMetrics::lb_degrade_selections("pin/inflight"));
    let weak = Arc::downgrade(&counter);
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let scrape = {
        let reached = Arc::clone(&reached);
        let resume = Arc::clone(&resume);
        std::thread::spawn(move || {
            GatewayMetrics::with_live_lb_degrade_counters(|| {
                reached.wait();
                resume.wait();
                handle.render()
            })
        })
    };
    reached.wait();
    // The scrape holds the cold-path lock. Selection must not acquire it.
    counter.record_selection();
    drop(counter);
    let pinned_during_render = weak.strong_count() > 0;
    resume.wait();
    let rendered = scrape.join().unwrap();
    assert!(pinned_during_render);
    assert!(rendered.contains("edgion_lb_degrade_selections_total{service=\"pin/inflight\"} 1"));
    assert!(weak.upgrade().is_none());
    assert!(!lock_degrade_counters().contains_key("pin/inflight"));
}

#[test]
fn degrade_counter_recreation_reuses_history_and_cleans_weak_entries_without_scrapes() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let first = GatewayMetrics::lb_degrade_selections("pin/recreated");
        let overlap = GatewayMetrics::lb_degrade_selections("pin/recreated");
        assert!(Arc::ptr_eq(&first, &overlap));
        first.record_selection();
        drop(first);
        overlap.record_selection();
        drop(overlap);
        assert!(!lock_degrade_counters().contains_key("pin/recreated"));
        let recreated = GatewayMetrics::lb_degrade_selections("pin/recreated");
        recreated.record_selection();
        let rendered = GatewayMetrics::with_live_lb_degrade_counters(|| handle.render());
        assert!(rendered.contains("edgion_lb_degrade_selections_total{service=\"pin/recreated\"} 3"));
        drop(recreated);
        assert!(!lock_degrade_counters().contains_key("pin/recreated"));
    });
}
