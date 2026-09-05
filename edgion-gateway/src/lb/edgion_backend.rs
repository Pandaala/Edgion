//! `EdgionBackend` — load balancer backend wrapper.
//!
//! Per-backend metrics (`conn_count`, `score`) live on the wrapper itself,
//! sharing the EP/EPS lifecycle with the backend list. All metric sharing
//! happens via `Arc<EdgionBackend>` — the type is intentionally `!Clone`
//! (auto-clone would silently fork atomic metric state).
//!
//! Implements `Deref<Target = pingora_load_balancing::Backend>` so callers
//! can read `addr` / `weight` / `ext` directly.
//

use crate::lb::ewma::score::{now_units, PeakEwmaScore};
use pingora_load_balancing::Backend;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
pub struct EdgionBackend {
    pub(crate) inner: Backend,
    conn_count: AtomicUsize,
    score: PeakEwmaScore,
    cb_inflight: AtomicUsize,
}

impl EdgionBackend {
    #[inline]
    pub fn new(inner: Backend) -> Self {
        Self {
            inner,
            conn_count: AtomicUsize::new(0),
            score: PeakEwmaScore::new(now_units()),
            cb_inflight: AtomicUsize::new(0),
        }
    }

    #[inline]
    pub fn inner(&self) -> &Backend {
        &self.inner
    }

    #[inline]
    pub fn into_inner(self) -> Backend {
        self.inner
    }

    /// Increment the in-flight connection counter at `connected_to_upstream`.
    ///
    /// Maintained by **both** scoring policies: LeastConn compares the count
    /// directly, and Peak-EWMA multiplies its latency estimate by `conn + 1`
    /// so a slow-but-quiet backend is not treated as free.
    #[inline]
    pub fn inc_conn(&self) {
        self.conn_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the in-flight connection counter, saturating at zero.
    ///
    /// Uses a CAS loop to avoid wraparound from a stray decrement. This matters
    /// when a request is cleaned up twice (retry path + logging path) despite
    /// the `take()` discipline on the ledger.
    #[inline]
    pub fn dec_conn(&self) {
        let mut cur = self.conn_count.load(Ordering::Relaxed);
        while cur > 0 {
            match self
                .conn_count
                .compare_exchange_weak(cur, cur - 1, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Read the current in-flight connection count.
    #[inline]
    pub fn conn(&self) -> u64 {
        self.conn_count.load(Ordering::Relaxed) as u64
    }

    /// Record a latency sample, in µs, observed at `now_units`.
    ///
    /// Peak-immediate: a sample worse than the current decayed estimate is
    /// adopted whole; a better one blends with the elapsed-derived weight. See
    /// `crate::lb::ewma::score` and `skills/12-algorithms/lb-peak-ewma.md`.
    #[inline]
    pub fn observe_latency(&self, latency_us: u64, now_units: u32) {
        self.score.observe(latency_us.min(u32::MAX as u64) as u32, now_units);
    }

    /// Store a synthetic sample only if it would raise the estimate; a value
    /// that would not be a peak is skipped entirely, leaving both the value and
    /// the decay anchor untouched.
    ///
    /// **This method bounds nothing** beyond the u32 µs field — callers own the
    /// policy ceiling. See [`PeakEwmaScore::observe_peak_only`] for the two
    /// call sites and their differing input contracts.
    #[inline]
    #[must_use = "the caller may need to know whether the peak was actually stored"]
    pub fn observe_peak_only(&self, sample_us: u64, now_units: u32) -> bool {
        self.score
            .observe_peak_only(sample_us.min(u32::MAX as u64) as u32, now_units)
    }

    /// Decayed latency estimate in µs, without the outstanding-load term.
    #[inline]
    pub fn latency_estimate(&self, now_units: u32) -> u32 {
        self.score.read(now_units)
    }

    /// Full P2C selection score: decayed latency times outstanding load.
    ///
    /// The `conn + 1` keeps an idle backend's cost tied to its latency so a
    /// slow-but-quiet backend is not treated as free, matching Tower/Linkerd's
    /// `rtt * (pending + 1)`.
    #[inline]
    pub fn score(&self, now_units: u32) -> u64 {
        (self.latency_estimate(now_units) as u64).saturating_mul(self.conn() + 1)
    }

    /// Try to reserve one circuit-breaker permit for this endpoint. Atomic
    /// check-and-increment: succeeds only while the current in-flight count is
    /// below `max`. This is the admit-time gate — one atomic op both checks the
    /// cap and reserves the slot, so concurrent admits can never exceed `max`.
    #[inline]
    pub fn try_acquire_cb_permit(&self, max: u32) -> bool {
        self.cb_inflight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |cur| {
                if (cur as u64) < max as u64 {
                    Some(cur + 1)
                } else {
                    None
                }
            })
            .is_ok()
    }

    /// Release one circuit-breaker permit, saturating at zero (CAS loop guards
    /// against a stray double-release, mirroring `dec_conn`).
    #[inline]
    pub fn release_cb_permit(&self) {
        let mut cur = self.cb_inflight.load(Ordering::Relaxed);
        while cur > 0 {
            match self
                .cb_inflight
                .compare_exchange_weak(cur, cur - 1, Ordering::AcqRel, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Current circuit-breaker in-flight count for this endpoint.
    #[inline]
    pub fn cb_inflight(&self) -> u64 {
        self.cb_inflight.load(Ordering::Relaxed) as u64
    }
}

impl Deref for EdgionBackend {
    type Target = Backend;

    #[inline]
    fn deref(&self) -> &Backend {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cb_permit_hard_caps_at_max() {
        let b = EdgionBackend::new(Backend::new("127.0.0.1:1").unwrap());
        assert!(b.try_acquire_cb_permit(2)); // 0 -> 1
        assert!(b.try_acquire_cb_permit(2)); // 1 -> 2
        assert!(!b.try_acquire_cb_permit(2)); // at cap, reject
        assert_eq!(b.cb_inflight(), 2);
        b.release_cb_permit(); // 2 -> 1
        assert!(b.try_acquire_cb_permit(2)); // 1 -> 2 again
    }

    #[test]
    fn cb_permit_release_saturates_at_zero() {
        let b = EdgionBackend::new(Backend::new("127.0.0.1:1").unwrap());
        b.release_cb_permit(); // stays 0, no wraparound
        assert_eq!(b.cb_inflight(), 0);
    }
}
