//! `EdgionBackend` — load balancer backend wrapper.
//!
//! Per-backend metrics (`conn_count`, `ewma_us`) live on the wrapper itself,
//! sharing the EP/EPS lifecycle with the backend list. All metric sharing
//! happens via `Arc<EdgionBackend>` — the type is intentionally `!Clone`
//! (auto-clone would silently fork atomic metric state).
//!
//! Implements `Deref<Target = pingora_load_balancing::Backend>` so callers
//! can read `addr` / `weight` / `ext` directly.
//

use pingora_load_balancing::Backend;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Initial EWMA value for new backends (1 ms in µs).
pub const INITIAL_EWMA_US: u64 = 1_000;

#[derive(Debug)]
pub struct EdgionBackend {
    pub(crate) inner: Backend,
    conn_count: AtomicUsize,
    ewma_us: AtomicU64,
    cb_inflight: AtomicUsize,
}

impl EdgionBackend {
    #[inline]
    pub fn new(inner: Backend) -> Self {
        Self {
            inner,
            conn_count: AtomicUsize::new(0),
            ewma_us: AtomicU64::new(INITIAL_EWMA_US),
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

    /// Increment the in-flight connection counter. Called by LeastConn when
    /// a request successfully connects (`connected_to_upstream`).
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

    /// Update EWMA using the percent-alpha formula. Reads alpha from the
    /// global EWMA config.
    pub fn update_ewma(&self, latency_us: u64) {
        let alpha = crate::lb::ewma::get_alpha() as u64;
        let mut current = self.ewma_us.load(Ordering::Relaxed);
        loop {
            // Widen the multiply to u128 so the intermediate products cannot
            // overflow u64. alpha is clamped to [0, 100], so 100 * u64::MAX
            // still fits in u128. The result is a weighted average bounded by
            // max(latency_us, current) <= u64::MAX, so the cast back is lossless.
            let new_ewma =
                ((alpha as u128 * latency_us as u128 + (100 - alpha) as u128 * current as u128) / 100) as u64;
            match self
                .ewma_us
                .compare_exchange_weak(current, new_ewma, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Read the current EWMA latency in microseconds.
    #[inline]
    pub fn ewma(&self) -> u64 {
        self.ewma_us.load(Ordering::Relaxed)
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
