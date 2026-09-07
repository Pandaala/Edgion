//! Per-backend retry budget accounting (`retryConstraint`).
//!
//! Each operation samples monotonic elapsed milliseconds once. Two epoch-tagged
//! buckets implement the interpolation `previous * (1 - elapsed_ratio) + current`.
//! An epoch, its attempts, and its retries occupy ONE atomic word. Reusing a
//! bucket publishes all three together; counters are never moved between buckets.
//! The optional minimum-rate floor uses the same protocol with its own interval.
//!
//! Admission remains a soft cap: the percentage/floor check and the subsequent
//! increments are separate. N concurrent deciders can over-admit by up to N-1;
//! epoch replacement adds no uncounted increment to a retained window. A delayed
//! operation belongs to its sampled epoch, never to an epoch sampled again on
//! contention. Missing history rejects a retry; a superseded bucket cannot be
//! resurrected, and an expired increment is not rolled back into another epoch.
//!
//! Gateway startup requires lock-free 128-bit atomics. Operations allocate
//! nothing, take no locks, and emit no tracing. CAS and snapshot validation are
//! lock-free, not wait-free: a caller can retry while other callers make progress.

use edgion_resources::resources::edgion_backend_traffic_policy::RetryConstraintConfig;
use portable_atomic::AtomicU128;
use std::sync::atomic::Ordering;
#[cfg(test)]
use std::sync::Arc;
use std::time::{Duration, Instant};

const COUNT_BITS: u32 = 40;
const COUNT_MAX: u64 = (1 << COUNT_BITS) - 1;
const EPOCH_MAX: u64 = (1 << 48) - 1;

/// COUNT_MAX is a sticky saturation sentinel, not a counter that may wrap.
/// Epoch zero denotes an unused bucket; real epochs start at one.
#[derive(Clone, Copy, Default)]
struct Bucket {
    epoch: u64,
    attempts: u64,
    retries: u64,
}

impl Bucket {
    fn unpack(value: u128) -> Self {
        Self {
            epoch: (value >> (2 * COUNT_BITS)) as u64,
            attempts: ((value >> COUNT_BITS) as u64) & COUNT_MAX,
            retries: (value as u64) & COUNT_MAX,
        }
    }

    fn pack(self) -> u128 {
        ((self.epoch as u128) << (2 * COUNT_BITS)) | ((self.attempts as u128) << COUNT_BITS) | self.retries as u128
    }

    fn saturated(self) -> bool {
        self.attempts == COUNT_MAX || self.retries == COUNT_MAX
    }
}

#[derive(Clone, Copy)]
enum Counter {
    Attempt,
    Retry,
}

#[derive(Clone, Copy)]
struct WindowPoint {
    epoch: u64,
    previous_weight: f64,
}

struct WindowSnapshot {
    current: Bucket,
    previous: Bucket,
}

impl WindowSnapshot {
    fn estimates(&self, point: WindowPoint) -> (f64, f64) {
        (
            self.previous.attempts as f64 * point.previous_weight + self.current.attempts as f64,
            self.previous.retries as f64 * point.previous_weight + self.current.retries as f64,
        )
    }
}

/// Fixed-grid windows: only the sampled epoch and its immediate predecessor
/// contribute, even if no operation has touched a bucket for many intervals.
struct EpochWindow {
    interval_ms: u64,
    buckets: [AtomicU128; 2],
    #[cfg(test)]
    hook: Option<Arc<dyn Fn(tests::Event) + Send + Sync>>,
}

impl EpochWindow {
    fn new(interval: Duration) -> Self {
        Self {
            // Validated GEP-2257 intervals are integral milliseconds, at most 1h.
            interval_ms: u64::try_from(interval.as_millis()).unwrap_or(u64::MAX).max(1),
            buckets: [AtomicU128::new(0), AtomicU128::new(0)],
            #[cfg(test)]
            hook: None,
        }
    }

    fn point_at(&self, now_ms: u64) -> Option<WindowPoint> {
        let epoch = (now_ms / self.interval_ms).checked_add(1)?;
        (epoch <= EPOCH_MAX).then(|| WindowPoint {
            epoch,
            previous_weight: 1.0 - (now_ms % self.interval_ms) as f64 / self.interval_ms as f64,
        })
    }

    fn snapshot_at(&self, point: WindowPoint) -> Option<WindowSnapshot> {
        let current_slot = &self.buckets[(point.epoch & 1) as usize];
        let previous_slot = &self.buckets[((point.epoch - 1) & 1) as usize];
        loop {
            // SeqCst orders this double collect with writes to BOTH slots.
            // Within an epoch counts only increase, and epochs never wrap, so
            // equal first/last words cannot hide ABA. Both words coexisted at
            // the middle load, including both numerator/denominator pairs.
            let first = current_slot.load(Ordering::SeqCst);
            #[cfg(test)]
            self.notify(tests::Event::AfterFirstLoad);
            let previous = previous_slot.load(Ordering::SeqCst);
            if current_slot.load(Ordering::SeqCst) != first {
                continue;
            }
            let mut current = Bucket::unpack(first);
            let mut previous = Bucket::unpack(previous);
            if current.epoch > point.epoch || previous.epoch > point.epoch - 1 {
                // A delayed decision cannot reconstruct its sampled history.
                return None;
            }
            if current.epoch != point.epoch {
                current = Bucket::default();
            }
            if previous.epoch != point.epoch - 1 {
                previous = Bucket::default();
            }
            if current.saturated() || previous.saturated() {
                return None;
            }
            #[cfg(test)]
            self.notify(tests::Event::AfterSnapshot);
            return Some(WindowSnapshot { current, previous });
        }
    }

    /// Replace an older slot and count this event in ONE CAS. A delayed writer
    /// can still count in its original epoch while that slot survives, but
    /// cannot overwrite a newer epoch. There is no decrement/rollback path.
    fn record_at(&self, point: WindowPoint, counter: Counter) -> bool {
        let slot = &self.buckets[(point.epoch & 1) as usize];
        let mut old = slot.load(Ordering::SeqCst);
        loop {
            let mut next = Bucket::unpack(old);
            if next.epoch > point.epoch {
                return false;
            }
            if next.epoch != point.epoch {
                next = Bucket {
                    epoch: point.epoch,
                    ..Bucket::default()
                };
            }
            if next.saturated() {
                return false;
            }
            match counter {
                Counter::Attempt => next.attempts += 1,
                Counter::Retry => next.retries += 1,
            }
            #[cfg(test)]
            self.notify(tests::Event::BeforeCas);
            match slot.compare_exchange_weak(old, next.pack(), Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => {
                    #[cfg(test)]
                    self.notify(tests::Event::AfterCas);
                    return true;
                }
                Err(actual) => old = actual,
            }
        }
    }

    #[cfg(test)]
    fn notify(&self, event: tests::Event) {
        if let Some(hook) = &self.hook {
            hook(event);
        }
    }
}

/// Shared per-target budget. Service and AI policy snapshots retain this Arc
/// across retries of one request; configuration replay retains it when unchanged.
pub struct RetryBudget {
    origin: Instant,
    percent: u8,
    budget_window: EpochWindow,
    min_retry: Option<(u32, EpochWindow)>,
    #[cfg(test)]
    clock: Option<Arc<tests::TestClock>>,
}

impl RetryBudget {
    /// Construct on the configuration path from a validated constraint.
    pub fn from_config(cfg: &RetryConstraintConfig) -> RetryBudget {
        let budget_interval =
            edgion_resources::duration::parse_duration(&cfg.budget.interval).unwrap_or(Duration::from_secs(1));
        let min_retry = cfg.min_retry_rate.as_ref().map(|floor| {
            let floor_interval =
                edgion_resources::duration::parse_duration(&floor.interval).unwrap_or(Duration::from_secs(1));
            (floor.count, EpochWindow::new(floor_interval))
        });
        RetryBudget {
            origin: Instant::now(),
            percent: cfg.budget.percent,
            budget_window: EpochWindow::new(budget_interval),
            min_retry,
            #[cfg(test)]
            clock: None,
        }
    }

    fn now_ms(&self) -> Option<u64> {
        #[cfg(test)]
        if let Some(clock) = &self.clock {
            return Some(clock.sample());
        }
        u64::try_from(Instant::now().saturating_duration_since(self.origin).as_millis()).ok()
    }

    /// Count every upstream attempt (original and retry) in its sampled epoch.
    pub fn on_attempt(&self) {
        if let Some(point) = self.now_ms().and_then(|now| self.budget_window.point_at(now)) {
            self.budget_window.record_at(point, Counter::Attempt);
        }
    }

    /// Permit and count a retry when either the percentage or the floor allows
    /// it. Saturated counters or unavailable sampled history deny the retry.
    pub fn try_retry(&self) -> bool {
        let Some(now) = self.now_ms() else {
            return false;
        };
        let Some(point) = self.budget_window.point_at(now) else {
            return false;
        };
        let Some(snapshot) = self.budget_window.snapshot_at(point) else {
            return false;
        };
        let (attempts, retries) = snapshot.estimates(point);
        let within_pct = retries < (self.percent as f64 / 100.0 * attempts).ceil();
        let floor = if let Some((count, window)) = &self.min_retry {
            let Some(point) = window.point_at(now) else {
                return false;
            };
            let Some(snapshot) = window.snapshot_at(point) else {
                return false;
            };
            Some((window, point, snapshot.estimates(point).1 < *count as f64))
        } else {
            None
        };
        if !within_pct && !floor.as_ref().is_some_and(|(_, _, allowed)| *allowed) {
            return false;
        }
        if !self.budget_window.record_at(point, Counter::Retry) {
            return false;
        }
        if let Some((window, point, _)) = floor {
            // If this shorter window expired during the commit, discard its
            // obsolete accounting. Never write into the new epoch or roll back
            // the already committed budget numerator.
            window.record_at(point, Counter::Retry);
        }
        true
    }
}

#[cfg(test)]
mod tests {
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
}
