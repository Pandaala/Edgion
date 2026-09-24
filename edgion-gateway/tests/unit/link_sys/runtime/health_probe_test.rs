use super::*;

#[test]
fn backoff_resets_on_success() {
    let mut b = ProbeBackoff::new(Duration::from_millis(500), Duration::from_secs(30));
    let _ = b.on_failure(1.0); // cur now 1s
    b.on_success();
    // After reset, the first failure interval is back to ~start.
    assert_eq!(b.on_failure(1.0), Duration::from_millis(500));
}

#[test]
fn backoff_doubles_and_clamps() {
    let mut b = ProbeBackoff::new(Duration::from_secs(10), Duration::from_secs(30));
    assert_eq!(b.on_failure(1.0), Duration::from_secs(10)); // returns cur, then cur=20
    assert_eq!(b.on_failure(1.0), Duration::from_secs(20)); // returns 20, then cur=30 (clamp)
    assert_eq!(b.on_failure(1.0), Duration::from_secs(30)); // clamped
    assert_eq!(b.on_failure(1.0), Duration::from_secs(30)); // stays clamped
                                                            // jitter > 1.0 at the clamped ceiling: returned interval must not exceed max
    assert_eq!(
        b.on_failure(1.2),
        Duration::from_secs(30),
        "jitter must not push returned interval past max"
    );
}

#[test]
fn backoff_applies_jitter_to_returned_interval_only() {
    let mut b = ProbeBackoff::new(Duration::from_secs(10), Duration::from_secs(30));
    let lo = b.on_failure(0.8);
    assert_eq!(lo, Duration::from_secs(8)); // 10 * 0.8, cur now 20
    let hi = b.on_failure(1.2);
    assert_eq!(hi, Duration::from_secs(24)); // 20 * 1.2, cur now 30
}

#[test]
fn flip_log_window_rate_limits() {
    let last = AtomicU64::new(0);
    assert!(flip_log_allowed_at(&last, 1_000, 5_000)); // first flip always logs
    assert!(!flip_log_allowed_at(&last, 2_000, 5_000)); // within window suppressed
    assert!(flip_log_allowed_at(&last, 6_000, 5_000)); // window elapsed
    assert_ne!(last.load(Ordering::Relaxed), 0); // never stores the sentinel
}

#[test]
fn flip_log_first_call_at_time_zero_logs() {
    let last = AtomicU64::new(0);
    assert!(flip_log_allowed_at(&last, 0, 5_000));
    assert_eq!(last.load(Ordering::Relaxed), 1); // now_ms.max(1)
}

#[test]
fn flip_log_clock_rollback_waits_for_the_full_window_after_catch_up() {
    let last = AtomicU64::new(0);
    assert!(flip_log_allowed_at(&last, 10_000, 5_000));

    assert!(!flip_log_allowed_at(&last, 9_000, 5_000));
    assert_eq!(last.load(Ordering::Relaxed), 10_000);

    assert!(!flip_log_allowed_at(&last, 10_000, 5_000));
    assert!(flip_log_allowed_at(&last, 15_000, 5_000));
}

#[test]
fn apply_result_flips_only_on_transition() {
    let h = PublishedHealth::new(false);
    let last = AtomicU64::new(0);
    assert!(apply_health_result(&h, "n", "test", &last, true, false)); // false->true
    assert!(h.load());
    assert!(!apply_health_result(&h, "n", "test", &last, true, false)); // already true, no flip
    assert!(apply_health_result(&h, "n", "test", &last, false, false)); // true->false
    assert!(!h.load());
}

#[test]
fn apply_result_single_winner_under_contention() {
    use std::sync::Arc;
    let h = Arc::new(PublishedHealth::new(false));
    let last = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();
    for _ in 0..16 {
        let h = Arc::clone(&h);
        let last = Arc::clone(&last);
        handles.push(std::thread::spawn(move || {
            apply_health_result(&h, "n", "test", &last, true, false)
        }));
    }
    let winners = handles.into_iter().map(|h| h.join().unwrap()).filter(|&w| w).count();
    assert_eq!(winners, 1, "exactly one thread may win the false->true flip");
}

#[test]
fn apply_result_upgrade_only_never_downgrades() {
    let h = PublishedHealth::new(true);
    let last = AtomicU64::new(0);
    // upgrade_only: a failure must NOT flip the flag to false.
    assert!(!apply_health_result(&h, "n", "test", &last, false, true));
    assert!(h.load());
    // but it can still recover a false flag.
    h.update(false, false);
    assert!(apply_health_result(&h, "n", "test", &last, true, true));
    assert!(h.load());
}

#[tokio::test(start_paused = true)]
async fn run_probe_loop_drives_flag_and_stops_on_shutdown() {
    use std::sync::atomic::AtomicUsize;
    // Scripted closure: call 1 -> true (recover), call 2 -> false (down),
    // call 3+ -> true. upgrade_only=false so it can both up- and downgrade.
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_c = Arc::clone(&calls);
    let probe = move || {
        let calls = Arc::clone(&calls_c);
        async move {
            let n = calls.fetch_add(1, Ordering::Relaxed);
            n != 1 // call index 1 (the 2nd call) reports unhealthy
        }
    };

    let healthy = Arc::new(PublishedHealth::new(false));
    let (sd_tx, sd_rx) = watch::channel(false);
    let cfg = ProbeConfig {
        name: "t".into(),
        system_type: "test",
        base_interval: Duration::from_secs(10),
        max_backoff: Duration::from_secs(30),
        probe_timeout: Duration::from_secs(5),
        upgrade_only: false,
    };
    let h = Arc::clone(&healthy);
    let task = tokio::spawn(run_probe_loop(cfg, h, sd_rx, probe));

    // Advance past the first interval -> recover.
    tokio::time::advance(Duration::from_secs(11)).await;
    tokio::task::yield_now().await;
    assert!(healthy.load(), "first probe should recover");

    // Next interval -> unhealthy.
    tokio::time::advance(Duration::from_secs(11)).await;
    tokio::task::yield_now().await;
    assert!(!healthy.load(), "second probe should downgrade");

    // Shutdown -> loop returns cleanly (no leak).
    sd_tx.send(true).unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    let joined = tokio::time::timeout(Duration::from_secs(1), task).await;
    assert!(joined.is_ok(), "probe loop must exit on shutdown");
}

#[tokio::test(start_paused = true)]
async fn run_probe_loop_exits_when_sender_dropped() {
    let healthy = Arc::new(PublishedHealth::new(true));
    let (sd_tx, sd_rx) = watch::channel(false);
    let cfg = ProbeConfig {
        name: "t".into(),
        system_type: "test",
        base_interval: Duration::from_secs(10),
        max_backoff: Duration::from_secs(30),
        probe_timeout: Duration::from_secs(5),
        upgrade_only: false,
    };
    let task = tokio::spawn(run_probe_loop(cfg, healthy, sd_rx, || async { true }));
    drop(sd_tx); // OTLP latent-shutdown path: all senders gone.
    tokio::task::yield_now().await; // give the task CPU to observe Err on changed()
    let joined = tokio::time::timeout(Duration::from_secs(1), task).await;
    assert!(joined.is_ok(), "probe loop must exit when the watch sender drops");
}
#[tokio::test(start_paused = true)]
async fn shutdown_interrupts_an_in_flight_probe() {
    let (tx, rx) = watch::channel(false);
    let healthy = Arc::new(PublishedHealth::new(false));
    let cfg = ProbeConfig {
        name: "test/otlp".into(),
        system_type: "otlp",
        base_interval: Duration::from_secs(10),
        max_backoff: Duration::from_secs(30),
        probe_timeout: Duration::from_secs(300),
        upgrade_only: false,
    };
    let (started, start_rx) = tokio::sync::oneshot::channel();
    let started = parking_lot::Mutex::new(Some(started));
    let task = tokio::spawn(run_probe_loop(cfg, healthy, rx, move || {
        if let Some(started) = started.lock().take() {
            let _ = started.send(());
        }
        std::future::pending::<bool>()
    }));
    start_rx.await.unwrap();
    tx.send_replace(true);
    tokio::time::timeout(Duration::from_millis(1), task)
        .await
        .unwrap()
        .unwrap();
}
