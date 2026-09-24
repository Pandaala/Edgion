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
#[path = "../../../../tests/unit/backends/policy/backend_traffic/retry_budget_test.rs"]
mod tests;
