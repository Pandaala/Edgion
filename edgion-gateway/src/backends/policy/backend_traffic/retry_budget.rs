//! Per-backend retry budget accounting (`retryConstraint`).
//!
//! Caps retry volume as a percentage of total request volume to a backend
//! over a sliding window, with an optional `minRetryRate` floor so low-traffic
//! backends can still retry a trickle when the percentage rounds to ~0.
//!
//! Both budget counters — the `attempts` denominator and the `retries`
//! numerator — share the SAME `budget.interval`, so they live in a single
//! [`BudgetWindow`] under ONE `window_start` and rotate together atomically
//! (both `current`→`previous` in the same rotation). This removes the
//! boundary skew that two independent windows would exhibit: with separate
//! `window_start`s set microseconds apart, the numerator and denominator
//! could rotate at slightly different instants and transiently distort the
//! ratio right at a window boundary. Sharing one rotation keeps the two
//! counters phase-locked, so the numerator/denominator ratio is always read
//! against the same window.
//!
//! The optional `minRetryRate` floor has its OWN interval (different from the
//! budget interval), so it stays a separate single-counter [`InterpWindow`].
//! Both types borrow the two-window interpolation *math* from
//! `SlidingWindowCounter` (`crate::link_sys::providers::webhook::runtime`) —
//! atomic, no background timer, lazily rotated on access. Unlike that type,
//! read (`estimate`) and increment (`incr`) are exposed separately because
//! [`RetryBudget::try_retry`] must consult the budget window's estimate (and
//! the floor window's estimate) before deciding whether to commit an
//! increment.
//!
//! Each public operation reads the wall clock exactly once (via
//! `now_epoch_secs`) and threads that instant through the rotation helper, so
//! rotation and interpolation observe a single, consistent `now`.
//!
//! **Concurrency is intentionally a soft cap.** `estimate()` and the
//! subsequent `incr()` are not one atomic step, and the decision in
//! `try_retry` combines the budget estimate with the floor estimate via OR,
//! so `SlidingWindowCounter`'s single-counter CAS guarantee does not apply
//! here. Under concurrency this may over-admit by up to N-1 concurrent
//! deciders. That is acceptable for a coarse anti-storm budget and avoids a
//! hot-path lock; do not try to make it exact — tests assert ranges, not
//! exact counts.

use edgion_resources::resources::edgion_backend_traffic_policy::RetryConstraintConfig;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Interpolated estimate for one (current, previous) counter pair, given the
/// window start after rotation and the single `now` already read by the
/// caller. Matches `SlidingWindowCounter`'s formula:
/// `previous * (1 - elapsed_ratio) + current`.
fn interp(now: u64, ws_after: u64, window_secs: u64, previous: u64, current: u64) -> f64 {
    let elapsed = now.saturating_sub(ws_after);
    let ratio = (elapsed as f64) / (window_secs as f64);
    (previous as f64) * (1.0 - ratio) + (current as f64)
}

/// The shared budget window carrying BOTH the attempts (denominator) and
/// retries (numerator) counters under ONE `window_start`, so they rotate
/// together atomically and never skew relative to each other at a boundary.
///
/// Rotation rolls both `current`→`previous` pairs in the same CAS-guarded
/// step, so a reader of `estimate_attempts` / `estimate_retries` always sees
/// the two counters on the same window phase.
struct BudgetWindow {
    window_secs: u64,
    window_start: AtomicU64,
    attempts_current: AtomicU64,
    attempts_previous: AtomicU64,
    retries_current: AtomicU64,
    retries_previous: AtomicU64,
}

impl BudgetWindow {
    fn new(window: Duration) -> Self {
        Self {
            window_secs: window.as_secs().max(1),
            window_start: AtomicU64::new(now_epoch_secs()),
            attempts_current: AtomicU64::new(0),
            attempts_previous: AtomicU64::new(0),
            retries_current: AtomicU64::new(0),
            retries_previous: AtomicU64::new(0),
        }
    }

    /// Rotate-on-read at the caller-provided `now`: if the window has expired,
    /// roll BOTH counter pairs (`current`→`previous`) and advance
    /// `window_start`. Returns the (possibly just rotated) window_start. A CAS
    /// ensures only one racing caller performs the rotation.
    fn rotate_at(&self, now: u64) -> u64 {
        let ws = self.window_start.load(Ordering::Acquire);

        if now >= ws + self.window_secs
            && self
                .window_start
                .compare_exchange(ws, now, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            let attempts = self.attempts_current.swap(0, Ordering::AcqRel);
            self.attempts_previous.store(attempts, Ordering::Release);
            let retries = self.retries_current.swap(0, Ordering::AcqRel);
            self.retries_previous.store(retries, Ordering::Release);
            return now;
        }

        self.window_start.load(Ordering::Acquire)
    }

    /// Rotate (both pairs, once) and increment the attempts denominator.
    fn incr_attempt(&self) {
        let now = now_epoch_secs();
        self.rotate_at(now);
        self.attempts_current.fetch_add(1, Ordering::Relaxed);
    }

    /// Rotate (both pairs, once) and return the interpolated attempts estimate.
    fn estimate_attempts(&self) -> f64 {
        let now = now_epoch_secs();
        let ws_after = self.rotate_at(now);
        let prev = self.attempts_previous.load(Ordering::Acquire);
        let curr = self.attempts_current.load(Ordering::Acquire);
        interp(now, ws_after, self.window_secs, prev, curr)
    }

    /// Rotate (both pairs, once) and increment the retries numerator.
    fn incr_retry(&self) {
        let now = now_epoch_secs();
        self.rotate_at(now);
        self.retries_current.fetch_add(1, Ordering::Relaxed);
    }

    /// Rotate (both pairs, once) and return the interpolated retries estimate.
    fn estimate_retries(&self) -> f64 {
        let now = now_epoch_secs();
        let ws_after = self.rotate_at(now);
        let prev = self.retries_previous.load(Ordering::Acquire);
        let curr = self.retries_current.load(Ordering::Acquire);
        interp(now, ws_after, self.window_secs, prev, curr)
    }
}

/// A single interpolated sliding-window counter, rotated lazily on access.
///
/// `estimate()` = `previous * (1 - elapsed_ratio) + current`, matching
/// `SlidingWindowCounter`'s interpolation formula, but exposed separately
/// from the increment so callers can read this window's estimate before
/// deciding whether to increment. Used for the `minRetryRate` floor, whose
/// interval differs from the shared budget interval.
struct InterpWindow {
    window_secs: u64,
    current: AtomicU64,
    previous: AtomicU64,
    window_start: AtomicU64,
}

impl InterpWindow {
    fn new(window: Duration) -> Self {
        Self {
            window_secs: window.as_secs().max(1),
            current: AtomicU64::new(0),
            previous: AtomicU64::new(0),
            window_start: AtomicU64::new(now_epoch_secs()),
        }
    }

    /// Rotate the window if it has expired, then return the interpolated
    /// estimate. Does not mutate `current` beyond the rotation swap.
    fn estimate(&self) -> f64 {
        let now = now_epoch_secs();
        let ws_after = self.rotate_at(now);
        let prev = self.previous.load(Ordering::Acquire);
        let curr = self.current.load(Ordering::Acquire);
        interp(now, ws_after, self.window_secs, prev, curr)
    }

    /// Rotate the window if it has expired, then increment `current`.
    fn incr(&self) {
        let now = now_epoch_secs();
        self.rotate_at(now);
        self.current.fetch_add(1, Ordering::Relaxed);
    }

    /// Rotate-on-read at the caller-provided `now`: if the window has expired,
    /// roll `current` into `previous` and advance `window_start`. Returns the
    /// (possibly just rotated) window_start. A CAS ensures only one racing
    /// caller performs the rotation.
    fn rotate_at(&self, now: u64) -> u64 {
        let ws = self.window_start.load(Ordering::Acquire);

        if now >= ws + self.window_secs
            && self
                .window_start
                .compare_exchange(ws, now, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            let current = self.current.swap(0, Ordering::AcqRel);
            self.previous.store(current, Ordering::Release);
            return now;
        }

        self.window_start.load(Ordering::Acquire)
    }
}

/// Per-service retry budget: caps retries as a percentage of total attempts
/// over `budget.interval`, with an optional `minRetryRate` floor.
///
/// The attempts denominator and retries numerator share one [`BudgetWindow`]
/// (same interval, phase-locked rotation → no boundary skew). The floor keeps
/// its own [`InterpWindow`] because its interval is independent.
///
/// See the module doc for the soft-cap concurrency note.
pub struct RetryBudget {
    percent: u8,
    budget_window: BudgetWindow,
    min_retry: Option<(u32, InterpWindow)>,
}

impl RetryBudget {
    /// Build a budget from a validated `RetryConstraintConfig`. Validation
    /// (edgion-resources) already guarantees `budget.interval` parses and is
    /// at least 1 second, and `minRetryRate.interval` parses and is
    /// positive, so parse failures here are unreachable in practice;
    /// `.max(1)` in the window constructors is the defensive floor.
    pub fn from_config(cfg: &RetryConstraintConfig) -> RetryBudget {
        let budget_interval =
            edgion_resources::duration::parse_duration(&cfg.budget.interval).unwrap_or(Duration::from_secs(1));
        let min_retry = cfg.min_retry_rate.as_ref().map(|floor| {
            let floor_interval =
                edgion_resources::duration::parse_duration(&floor.interval).unwrap_or(Duration::from_secs(1));
            (floor.count, InterpWindow::new(floor_interval))
        });
        RetryBudget {
            percent: cfg.budget.percent,
            budget_window: BudgetWindow::new(budget_interval),
            min_retry,
        }
    }

    /// Denominator +1. Call on EVERY upstream attempt (original and retry).
    pub fn on_attempt(&self) {
        self.budget_window.incr_attempt();
    }

    /// Try to permit one more retry. Returns true (and commits the numerator
    /// increment) if either the percentage cap or the floor allows it.
    pub fn try_retry(&self) -> bool {
        let attempts = self.budget_window.estimate_attempts();
        let cap_pct = (self.percent as f64 / 100.0 * attempts).ceil();
        let within_pct = self.budget_window.estimate_retries() < cap_pct;
        let within_floor = match &self.min_retry {
            Some((count, w)) => w.estimate() < *count as f64,
            None => false,
        };
        if within_pct || within_floor {
            self.budget_window.incr_retry();
            if let Some((_, w)) = &self.min_retry {
                w.incr();
            }
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_backend_traffic_policy::{BudgetDetails, RateThreshold};

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
        let cfg = mk_cfg(20, "10s", Some((10, "1s")));
        let b = RetryBudget::from_config(&cfg);
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
        let cfg = mk_cfg(20, "10s", None);
        let b = RetryBudget::from_config(&cfg);
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
}
