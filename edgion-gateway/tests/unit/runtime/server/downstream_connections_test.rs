use super::*;
use futures::FutureExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::atomic::AtomicBool;

fn limited(limit: u64) -> Arc<ConnectionCounter> {
    Arc::new(ConnectionCounter::new(NonZeroU64::new(limit)))
}

#[test]
fn acquire_returns_ordinal_including_itself_and_slot_drop_releases() {
    let counter = Arc::new(ConnectionCounter::unlimited());
    let (first, first_ordinal) = counter.acquire();
    let (second, second_ordinal) = counter.acquire();
    assert_eq!((first_ordinal, second_ordinal), (1, 2));
    assert_eq!(counter.active(), 2);
    drop(first);
    assert_eq!(counter.active(), 1);
    let (_third, third_ordinal) = counter.acquire();
    assert_eq!(third_ordinal, 2, "a released slot is reused by the next ordinal");
    drop(second);
    assert_eq!(counter.active(), 1);
}

#[test]
fn try_acquire_stops_at_the_cap_and_a_rejection_never_changes_the_count() {
    let counter = limited(2);
    let first = counter.try_acquire().expect("below cap");
    let _second = counter.try_acquire().expect("at cap");
    assert_eq!(counter.active(), 2);
    for _ in 0..3 {
        assert!(counter.try_acquire().is_none());
        assert_eq!(counter.active(), 2, "rejected attempts must not inflate the count");
    }
    drop(first);
    assert!(counter.try_acquire().is_some(), "a released slot admits again");
}

#[test]
fn unlimited_try_acquire_always_admits() {
    let counter = Arc::new(ConnectionCounter::new(None));
    let slots: Vec<_> = (0..100).map(|_| counter.try_acquire().unwrap()).collect();
    assert_eq!(counter.active(), 100);
    drop(slots);
    assert_eq!(counter.active(), 0);
}

#[test]
fn concurrent_try_acquire_never_exceeds_the_cap() {
    const CAP: u64 = 8;
    let counter = limited(CAP);
    let admitted = Arc::new(AtomicU64::new(0));
    let peak = Arc::new(AtomicU64::new(0));
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let counter = Arc::clone(&counter);
            let admitted = Arc::clone(&admitted);
            let peak = Arc::clone(&peak);
            std::thread::spawn(move || {
                for _ in 0..2_000 {
                    if let Some(slot) = counter.try_acquire() {
                        admitted.fetch_add(1, Ordering::Relaxed);
                        peak.fetch_max(counter.active(), Ordering::Relaxed);
                        drop(slot);
                    }
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(admitted.load(Ordering::Relaxed) > 0);
    assert!(peak.load(Ordering::Relaxed) <= CAP);
    assert_eq!(counter.active(), 0);
}

#[test]
fn concurrent_ordinals_never_admit_more_than_the_limit() {
    // Mirrors ConnectionLimit: admit when ordinal <= max, else release.
    const MAX: u64 = 4;
    let counter = Arc::new(ConnectionCounter::unlimited());
    let held = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let counter = Arc::clone(&counter);
            let held = Arc::clone(&held);
            std::thread::spawn(move || {
                for _ in 0..200 {
                    let (slot, ordinal) = counter.acquire();
                    if ordinal <= MAX {
                        held.lock().push(slot);
                    }
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let admitted = held.lock().len() as u64;
    assert!(admitted <= MAX, "admitted {admitted} live slots with max {MAX}");
    assert_eq!(counter.active(), admitted);
}

#[tokio::test]
async fn fatal_policy_counts_nothing_and_never_runs_the_chain() {
    let global = limited(10);
    let listener = Arc::new(ConnectionCounter::unlimited());
    let ran = AtomicBool::new(false);
    let permit = admit_downstream_connection(global_metrics(), true, &global, &listener, |_| {
        ran.store(true, Ordering::Relaxed);
        assert_eq!(
            global.active() + listener.active(),
            0,
            "nothing may be counted while fatal"
        );
        async { true }
    })
    .await;
    assert!(permit.is_none());
    assert!(
        !ran.load(Ordering::Relaxed),
        "the chain must not run while the policy is fatal"
    );
    assert_eq!((global.active(), listener.active()), (0, 0));
}

#[tokio::test]
async fn global_rejection_takes_no_listener_slot_and_skips_the_chain() {
    let global = limited(1);
    let listener = Arc::new(ConnectionCounter::unlimited());
    let _occupied = global.try_acquire().unwrap();
    let ran = AtomicBool::new(false);
    let permit = admit_downstream_connection(global_metrics(), false, &global, &listener, |_| {
        ran.store(true, Ordering::Relaxed);
        async { true }
    })
    .await;
    assert!(permit.is_none());
    assert!(!ran.load(Ordering::Relaxed));
    assert_eq!(global.active(), 1);
    assert_eq!(listener.active(), 0);
}

#[tokio::test]
async fn chain_receives_the_listener_ordinal_and_denial_releases_both_slots() {
    let global = limited(10);
    let listener = Arc::new(ConnectionCounter::unlimited());
    let (_existing, _) = listener.acquire();
    let ran = AtomicBool::new(false);
    let permit = admit_downstream_connection(global_metrics(), false, &global, &listener, |ordinal| {
        assert_eq!(ordinal, 2);
        assert_eq!(global.active(), 1, "global slot is held while the chain runs");
        ran.store(true, Ordering::Relaxed);
        async { false }
    })
    .await;
    assert!(ran.load(Ordering::Relaxed));
    assert!(permit.is_none());
    assert_eq!(global.active(), 0);
    assert_eq!(listener.active(), 1);
}

#[tokio::test]
async fn admitted_permit_holds_both_slots_until_dropped() {
    let global = limited(10);
    let listener = Arc::new(ConnectionCounter::unlimited());
    let permit = admit_downstream_connection(global_metrics(), false, &global, &listener, |_| async { true })
        .await
        .expect("admitted");
    assert_eq!((global.active(), listener.active()), (1, 1));
    // Pingora owns the permit through the accepted stream; dropping the
    // stream drops it.
    let wrapped = ConnectionPermit::new(permit);
    drop(wrapped);
    assert_eq!((global.active(), listener.active()), (0, 0));
}

#[test]
fn cancelling_admission_mid_chain_releases_both_slots() {
    let global = limited(10);
    let listener = Arc::new(ConnectionCounter::unlimited());
    let held_while_pending = AtomicU64::new(0);
    let admission = admit_downstream_connection(global_metrics(), false, &global, &listener, |_| {
        held_while_pending.store(global.active() + listener.active(), Ordering::Relaxed);
        std::future::pending::<bool>()
    });
    // Polling once reaches the pending chain with both slots taken; dropping the
    // future (as a shutdown `select!` does) must release them.
    assert!(admission.now_or_never().is_none());
    assert_eq!(held_while_pending.load(Ordering::Relaxed), 2);
    assert_eq!((global.active(), listener.active()), (0, 0));
}

fn listener_filter(global: &Arc<ConnectionCounter>, listener: &Arc<ConnectionCounter>) -> ListenerAdmissionFilter {
    let key = edgion_resources::ListenerKey::new("127.0.0.1:18496".parse().unwrap(), "TCP");
    ListenerAdmissionFilter::new(
        Arc::new(ProxyProtocolPolicyRuntime::new()),
        Arc::clone(global),
        Arc::clone(listener),
        StreamPluginConnectionFilter::new(key, 18496),
    )
}

#[tokio::test]
async fn listener_filter_admits_with_the_listener_ordinal_and_holds_both_slots() {
    let global = limited(4);
    let listener = Arc::new(ConnectionCounter::unlimited());
    let (_earlier, _) = listener.acquire();
    let filter = listener_filter(&global, &listener);
    let seen = AtomicU64::new(0);
    let admission = filter
        .admit_with(|ordinal| {
            seen.store(ordinal, Ordering::Relaxed);
            async { true }
        })
        .await;
    // The chain sees the listener's ordinal (an earlier listener socket is
    // live), not the process count.
    assert_eq!(seen.load(Ordering::Relaxed), 2);
    let ConnectionAdmission::Accept(Some(permit)) = admission else {
        panic!("an accepting chain must admit with a permit");
    };
    assert_eq!((global.active(), listener.active()), (1, 2));
    drop(permit);
    assert_eq!((global.active(), listener.active()), (0, 1));
}

#[tokio::test]
async fn listener_filter_enforces_the_process_cap_before_the_chain() {
    let global = limited(1);
    let listener = Arc::new(ConnectionCounter::unlimited());
    let _occupied = global.try_acquire().unwrap();
    let filter = listener_filter(&global, &listener);
    let ran = AtomicBool::new(false);
    let admission = filter
        .admit_with(|_| {
            ran.store(true, Ordering::Relaxed);
            async { true }
        })
        .await;
    assert!(matches!(admission, ConnectionAdmission::Reject));
    assert!(
        !ran.load(Ordering::Relaxed),
        "an over-cap socket must not reach the chain"
    );
    assert_eq!((global.active(), listener.active()), (1, 0));
}

#[tokio::test]
async fn listener_filter_releases_slots_when_its_real_chain_rejects() {
    // No publication exists for this listener, so the real Stage-1 view is
    // `Unavailable` and the chain rejects after the slots are taken.
    let global = limited(4);
    let listener = Arc::new(ConnectionCounter::unlimited());
    let filter = listener_filter(&global, &listener);
    let peer: SocketAddr = "192.0.2.10:40000".parse().unwrap();
    assert!(matches!(filter.admit(Some(&peer)).await, ConnectionAdmission::Reject));
    assert_eq!((global.active(), listener.active()), (0, 0));
}

#[tokio::test]
async fn listener_filter_decision_only_query_fails_closed() {
    let global = limited(4);
    let listener = Arc::new(ConnectionCounter::unlimited());
    let filter = listener_filter(&global, &listener);
    let peer: SocketAddr = "192.0.2.10:40002".parse().unwrap();
    assert!(!filter.should_accept(Some(&peer)).await);
    assert_eq!((global.active(), listener.active()), (0, 0));
}

#[test]
fn gauge_counts_admitted_permits_and_rejections_record_global_reason() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let metrics: &'static GatewayMetrics = Box::leak(Box::new(
            crate::observe::metrics::registry::tests::fresh_gateway_metrics(),
        ));
        let gauge = |value: u64| format!("edgion_downstream_connections_active {value}");
        let global = limited(1);
        let listener = Arc::new(ConnectionCounter::unlimited());

        let permit = admit_downstream_connection(metrics, false, &global, &listener, |_| async { true })
            .now_or_never()
            .flatten()
            .expect("admitted");
        assert!(handle.render().lines().any(|line| line == gauge(1)));

        // Over the cap: counted as a global rejection, gauge unchanged.
        assert!(
            admit_downstream_connection(metrics, false, &global, &listener, |_| async { true })
                .now_or_never()
                .flatten()
                .is_none()
        );
        let rendered = handle.render();
        assert!(rendered.lines().any(|line| line == gauge(1)));
        assert!(rendered
            .lines()
            .any(|line| line == "edgion_downstream_connections_rejected_total{reason=\"global\"} 1"));

        // A chain denial is not admitted and never touches the gauge.
        drop(permit);
        assert!(
            admit_downstream_connection(metrics, false, &global, &listener, |_| async { false })
                .now_or_never()
                .flatten()
                .is_none()
        );
        assert!(handle.render().lines().any(|line| line == gauge(0)));
    });
}

#[tokio::test]
async fn each_listener_filter_owns_its_counter_and_shares_the_process_counter() {
    let global = limited(4);
    let key = |port: u16| edgion_resources::ListenerKey::new(format!("127.0.0.1:{port}").parse().unwrap(), "TCP");
    let first = ListenerAdmissionFilter::for_listener(
        Arc::new(ProxyProtocolPolicyRuntime::new()),
        Arc::clone(&global),
        StreamPluginConnectionFilter::new(key(18497), 18497),
    );
    let second = ListenerAdmissionFilter::for_listener(
        Arc::new(ProxyProtocolPolicyRuntime::new()),
        Arc::clone(&global),
        StreamPluginConnectionFilter::new(key(18498), 18498),
    );
    let mut ordinals = Vec::new();
    let mut permits = Vec::new();
    for filter in [&first, &first, &second] {
        let seen = AtomicU64::new(0);
        let admission = filter
            .admit_with(|ordinal| {
                seen.store(ordinal, Ordering::Relaxed);
                async { true }
            })
            .await;
        ordinals.push(seen.load(Ordering::Relaxed));
        permits.push(admission);
    }
    // Listener ordinals are per listener; the process counter sees all three.
    assert_eq!(ordinals, [1, 2, 1]);
    assert_eq!(global.active(), 3);
    drop(permits);
    assert_eq!(global.active(), 0);
}
