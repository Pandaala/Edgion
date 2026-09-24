use super::*;
use edgion_resources::resources::edgion_backend_traffic_policy::{BudgetDetails, RateThreshold};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::Barrier;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Event {
    BeforeCas,
    AfterCas,
    AfterFirstLoad,
    AfterSnapshot,
}

#[derive(Default)]
pub(super) struct TestClock {
    now_ms: AtomicU64,
    samples: AtomicUsize,
}

impl TestClock {
    pub(super) fn sample(&self) -> u64 {
        self.samples.fetch_add(1, Ordering::SeqCst);
        self.now_ms.load(Ordering::SeqCst)
    }

    fn set(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }
}

fn frozen_budget(percent: u8, interval: &str, floor: Option<(u32, &str)>) -> RetryBudget {
    let mut budget = RetryBudget::from_config(&mk_cfg(percent, interval, floor));
    budget.clock = Some(Arc::new(TestClock::default()));
    budget
}

fn point(window: &EpochWindow, now: u64) -> WindowPoint {
    window.point_at(now).unwrap()
}

fn snapshot(window: &EpochWindow, now: u64) -> WindowSnapshot {
    window.snapshot_at(point(window, now)).unwrap()
}

fn seed(window: &EpochWindow, now: u64, attempts: u64, retries: u64) {
    let epoch = point(window, now).epoch;
    window.buckets[(epoch & 1) as usize].store(
        Bucket {
            epoch,
            attempts,
            retries,
        }
        .pack(),
        Ordering::SeqCst,
    );
}

/// Pause exactly one operation at the selected event. Other operations
/// continue, proving that a paused writer does not own a rotation lock.
fn pause_once(window: &mut EpochWindow, event: Event) -> (Arc<Barrier>, Arc<Barrier>) {
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let reached_hook = reached.clone();
    let resume_hook = resume.clone();
    let first = AtomicBool::new(true);
    window.hook = Some(Arc::new(move |observed| {
        if observed == event && first.swap(false, Ordering::SeqCst) {
            reached_hook.wait();
            resume_hook.wait();
        }
    }));
    (reached, resume)
}

fn mk_cfg(percent: u8, budget_interval: &str, floor: Option<(u32, &str)>) -> RetryConstraintConfig {
    RetryConstraintConfig {
        budget: BudgetDetails {
            percent,
            interval: budget_interval.to_string(),
        },
        min_retry_rate: floor.map(|(count, interval)| RateThreshold {
            count,
            interval: interval.to_string(),
        }),
    }
}

#[test]
fn floor_permits_trickle_at_low_volume() {
    // percent 20, but only a few attempts → pct cap ~0/1; floor count 10/1s lets a trickle.
    let b = frozen_budget(20, "10s", Some((10, "1s")));
    b.on_attempt(); // one original attempt
                    // pct cap = ceil(0.2*1)=1 → first retry allowed by pct; subsequent by floor up to 10.
    let mut allowed = 0;
    for _ in 0..20 {
        b.on_attempt();
        if b.try_retry() {
            allowed += 1;
        }
    }
    assert!(allowed >= 10, "floor must let ~10 retries through, got {allowed}");
    assert!(allowed < 20, "must not allow unbounded, got {allowed}");
}

#[test]
fn percent_cap_uses_total_attempts_denominator() {
    // No floor → only the percentage governs. 100 attempts, 20% → ~20 retries.
    let b = frozen_budget(20, "10s", None);
    let mut allowed = 0;
    for _ in 0..100 {
        b.on_attempt();
        if b.try_retry() {
            allowed += 1;
            b.on_attempt();
        }
    }
    // retries counted in denominator too → invariant retries <= 20% of total, soft.
    assert!((15..=30).contains(&allowed), "≈20% of total attempts, got {allowed}");
}

#[test]
fn milliseconds_and_fixed_grid_expire_old_history() {
    let b = frozen_budget(100, "1s500ms", Some((2, "500ms")));
    assert_eq!(b.budget_window.interval_ms, 1500);
    let floor = &b.min_retry.as_ref().unwrap().1;
    assert_eq!(floor.interval_ms, 500);
    b.on_attempt();
    assert!(b.try_retry());
    let at_boundary = snapshot(&b.budget_window, 1500);
    assert_eq!(at_boundary.current.attempts, 0);
    assert_eq!(at_boundary.previous.attempts, 1);
    assert_eq!(at_boundary.estimates(point(&b.budget_window, 2250)), (0.5, 0.5));
    for now in [3000, 3750, 150_000] {
        let s = snapshot(&b.budget_window, now);
        assert_eq!((s.previous.attempts, s.previous.retries), (0, 0));
        assert_eq!(s.estimates(point(&b.budget_window, now)), (0.0, 0.0));
    }
    assert_eq!(snapshot(floor, 1000).estimates(point(floor, 1000)), (0.0, 0.0));
    // First post-idle event stays at the original grid, not a new grid
    // anchored at the time of this access.
    b.clock.as_ref().unwrap().set(3750);
    b.on_attempt();
    assert_eq!(snapshot(&b.budget_window, 4500).previous.attempts, 1);
}

#[test]
fn floor_reopens_after_two_subsecond_intervals() {
    let b = frozen_budget(0, "10s", Some((2, "500ms")));
    let clock = b.clock.as_ref().unwrap();
    assert!(b.try_retry());
    assert!(b.try_retry());
    assert!(!b.try_retry());
    clock.set(500);
    assert!(
        !b.try_retry(),
        "the previous floor still has full weight at the boundary"
    );
    clock.set(1000);
    assert!(b.try_retry());
    assert!(b.try_retry());
    assert!(!b.try_retry());
}

#[test]
fn public_operations_sample_once_for_success_and_denial() {
    for floor in [None, Some((1, "500ms"))] {
        let b = frozen_budget(100, "10s", floor);
        let clock = b.clock.as_ref().unwrap();
        b.on_attempt();
        assert_eq!(clock.samples.load(Ordering::SeqCst), 1);
        assert!(b.try_retry());
        assert_eq!(clock.samples.load(Ordering::SeqCst), 2);
        assert!(!b.try_retry());
        assert_eq!(clock.samples.load(Ordering::SeqCst), 3);
    }
}

#[test]
fn published_new_epoch_never_moves_a_racing_increment_to_previous() {
    let mut window = EpochWindow::new(Duration::from_secs(1));
    seed(&window, 0, 10, 2);
    let (reached, resume) = pause_once(&mut window, Event::AfterCas);
    std::thread::scope(|scope| {
        let rotator = scope.spawn(|| window.record_at(point(&window, 1000), Counter::Attempt));
        reached.wait();
        assert!(window.record_at(point(&window, 1000), Counter::Attempt));
        assert!(window.record_at(point(&window, 1000), Counter::Retry));
        let s = snapshot(&window, 1000);
        assert_eq!((s.current.attempts, s.current.retries), (2, 1));
        assert_eq!((s.previous.attempts, s.previous.retries), (10, 2));
        resume.wait();
        assert!(rotator.join().unwrap());
    });
    assert_eq!(snapshot(&window, 1000).current.attempts, 2);
}

#[test]
fn delayed_attempt_cannot_replace_a_newer_epoch_or_resample_time() {
    let mut b = frozen_budget(100, "1s", None);
    let (reached, resume) = pause_once(&mut b.budget_window, Event::BeforeCas);
    let clock = b.clock.as_ref().unwrap();
    std::thread::scope(|scope| {
        let delayed = scope.spawn(|| b.on_attempt());
        reached.wait();
        clock.set(2000);
        b.on_attempt();
        resume.wait();
        delayed.join().unwrap();
    });
    assert_eq!(clock.samples.load(Ordering::SeqCst), 2);
    let s = snapshot(&b.budget_window, 2000);
    assert_eq!(s.current.attempts, 1);
    assert_eq!(s.previous.attempts, 0);
}

#[test]
fn delayed_attempt_stays_in_its_sampled_epoch_while_retained() {
    let mut b = frozen_budget(100, "1s", None);
    let (reached, resume) = pause_once(&mut b.budget_window, Event::BeforeCas);
    std::thread::scope(|scope| {
        let delayed = scope.spawn(|| b.on_attempt());
        reached.wait();
        b.clock.as_ref().unwrap().set(1000);
        b.on_attempt();
        resume.wait();
        delayed.join().unwrap();
    });
    let s = snapshot(&b.budget_window, 1000);
    assert_eq!((s.current.attempts, s.previous.attempts), (1, 1));
}

#[test]
fn competing_first_writers_retry_cas_without_losing_counts_or_resampling() {
    let mut b = frozen_budget(100, "1s", None);
    let (reached, resume) = pause_once(&mut b.budget_window, Event::BeforeCas);
    std::thread::scope(|scope| {
        let delayed = scope.spawn(|| b.on_attempt());
        reached.wait();
        b.on_attempt();
        resume.wait();
        delayed.join().unwrap();
    });
    assert_eq!(snapshot(&b.budget_window, 0).current.attempts, 2);
    assert_eq!(b.clock.as_ref().unwrap().samples.load(Ordering::SeqCst), 2);
}

#[test]
fn snapshot_retries_when_current_changes_between_reads() {
    let mut window = EpochWindow::new(Duration::from_secs(1));
    seed(&window, 0, 10, 2);
    let (reached, resume) = pause_once(&mut window, Event::AfterFirstLoad);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| snapshot(&window, 1000));
        reached.wait();
        assert!(window.record_at(point(&window, 1000), Counter::Attempt));
        assert!(window.record_at(point(&window, 1000), Counter::Retry));
        resume.wait();
        let s = reader.join().unwrap();
        assert_eq!((s.current.attempts, s.current.retries), (1, 1));
        assert_eq!((s.previous.attempts, s.previous.retries), (10, 2));
    });
}

#[test]
fn missing_sampled_history_rejects_without_resurrection() {
    let mut b = frozen_budget(100, "1s", None);
    b.on_attempt();
    let (reached, resume) = pause_once(&mut b.budget_window, Event::AfterSnapshot);
    std::thread::scope(|scope| {
        let delayed = scope.spawn(|| b.try_retry());
        reached.wait();
        b.clock.as_ref().unwrap().set(2000);
        b.on_attempt();
        resume.wait();
        assert!(!delayed.join().unwrap());
    });
    let s = snapshot(&b.budget_window, 2000);
    assert_eq!((s.current.attempts, s.current.retries), (1, 0));
    assert!(b.budget_window.snapshot_at(point(&b.budget_window, 0)).is_none());
    assert_eq!(b.clock.as_ref().unwrap().samples.load(Ordering::SeqCst), 3);
}

#[test]
fn missing_floor_history_denies_even_when_percentage_allows() {
    let mut b = frozen_budget(100, "10s", Some((1, "500ms")));
    b.on_attempt();
    let floor = &b.min_retry.as_ref().unwrap().1;
    let (reached, resume) = pause_once(&mut b.budget_window, Event::AfterSnapshot);
    std::thread::scope(|scope| {
        let delayed = scope.spawn(|| b.try_retry());
        reached.wait();
        assert!(floor.record_at(point(floor, 1000), Counter::Retry));
        resume.wait();
        assert!(!delayed.join().unwrap());
    });
    assert_eq!(snapshot(&b.budget_window, 0).current.retries, 0);
}

#[test]
fn expired_floor_commit_never_rolls_back_or_charges_new_window() {
    let mut b = frozen_budget(100, "10s", Some((1, "500ms")));
    b.on_attempt();
    let (reached, resume) = pause_once(&mut b.budget_window, Event::AfterCas);
    let floor = &b.min_retry.as_ref().unwrap().1;
    std::thread::scope(|scope| {
        let delayed = scope.spawn(|| b.try_retry());
        reached.wait();
        assert!(floor.record_at(point(floor, 1000), Counter::Retry));
        resume.wait();
        assert!(delayed.join().unwrap());
    });
    assert_eq!(snapshot(&b.budget_window, 0).current.retries, 1);
    let s = snapshot(floor, 1000);
    assert_eq!((s.current.retries, s.previous.retries), (1, 0));
    assert_eq!(b.clock.as_ref().unwrap().samples.load(Ordering::SeqCst), 2);
}

#[test]
fn counter_saturation_denies_until_history_expires_without_wrap() {
    let b = frozen_budget(100, "1s", Some((10, "1s")));
    seed(&b.budget_window, 0, COUNT_MAX - 1, 0);
    b.on_attempt();
    b.on_attempt();
    assert!(!b.try_retry(), "floor must not override saturated budget history");
    let raw = b.budget_window.buckets[1].load(Ordering::SeqCst);
    let bucket = Bucket::unpack(raw);
    assert_eq!((bucket.epoch, bucket.attempts, bucket.retries), (1, COUNT_MAX, 0));
    b.clock.as_ref().unwrap().set(1000);
    assert!(!b.try_retry());
    b.clock.as_ref().unwrap().set(2000);
    b.on_attempt();
    assert!(b.try_retry());
    let floor = &b.min_retry.as_ref().unwrap().1;
    seed(floor, 2000, 0, COUNT_MAX);
    assert!(!b.try_retry(), "saturated floor also fails closed");
}

#[test]
fn retry_saturation_is_sticky_and_never_carries_into_attempts() {
    let window = EpochWindow::new(Duration::from_secs(1));
    seed(&window, 0, 50, COUNT_MAX - 1);
    assert!(window.record_at(point(&window, 0), Counter::Retry));
    assert!(!window.record_at(point(&window, 0), Counter::Retry));
    let bucket = Bucket::unpack(window.buckets[1].load(Ordering::SeqCst));
    assert_eq!((bucket.attempts, bucket.retries), (50, COUNT_MAX));
}

#[test]
fn epoch_overflow_denies_without_reusing_an_old_identifier() {
    let b = frozen_budget(100, "1s", Some((1, "1ms")));
    let clock = b.clock.as_ref().unwrap();
    let floor = &b.min_retry.as_ref().unwrap().1;
    clock.set(EPOCH_MAX - 1);
    b.on_attempt();
    assert!(b.try_retry());
    assert_eq!(snapshot(floor, EPOCH_MAX - 1).current.epoch, EPOCH_MAX);
    clock.set(EPOCH_MAX);
    assert!(!b.try_retry(), "floor epoch exhausted");
    clock.set(u64::MAX);
    b.on_attempt();
    assert!(!b.try_retry(), "budget epoch exhausted");
    assert!(floor.point_at(u64::MAX).is_none());
}

#[test]
fn concurrent_deciders_preserve_the_documented_soft_bound() {
    const DECIDERS: usize = 8;
    let mut b = frozen_budget(100, "1s", None);
    b.on_attempt();
    let barrier = Arc::new(Barrier::new(DECIDERS));
    b.budget_window.hook = Some(Arc::new(move |event| {
        if event == Event::AfterSnapshot {
            barrier.wait();
        }
    }));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..DECIDERS).map(|_| scope.spawn(|| b.try_retry())).collect();
        for handle in handles {
            assert!(handle.join().unwrap());
        }
    });
    b.budget_window.hook = None;
    assert_eq!(snapshot(&b.budget_window, 0).current.retries, DECIDERS as u64);
    assert!(!b.try_retry());
}

#[test]
fn concurrent_attempts_and_retries_keep_exact_accounting() {
    const WORKERS: usize = 8;
    const OPERATIONS: usize = 1000;
    let b = frozen_budget(20, "10s", Some((5, "1s")));
    let admitted = AtomicU64::new(0);
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                for _ in 0..OPERATIONS {
                    b.on_attempt();
                    if b.try_retry() {
                        admitted.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    let s = snapshot(&b.budget_window, 0);
    let total_admitted = admitted.load(Ordering::Relaxed);
    assert_eq!(s.current.attempts, (WORKERS * OPERATIONS) as u64);
    assert_eq!(s.current.retries, total_admitted);
    let floor = &b.min_retry.as_ref().unwrap().1;
    assert_eq!(snapshot(floor, 0).current.retries, total_admitted);
    assert!(total_admitted <= (WORKERS * OPERATIONS / 5 + WORKERS - 1) as u64);
}
