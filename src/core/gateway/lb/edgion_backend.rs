//! `EdgionBackend` — load balancer backend wrapper.
//!
//! Per-backend metrics (`conn_count`, `ewma_us`) live on the wrapper itself,
//! sharing the EP/EPS lifecycle with the backend list rather than living in a
//! side `runtime_state` DashMap. All metric sharing happens via
//! `Arc<EdgionBackend>` — the type is intentionally `!Clone` (auto-clone
//! would silently fork atomic metric state).
//!
//! Implements `Deref<Target = pingora_load_balancing::Backend>` so existing
//! call sites that read `addr` / `weight` / `ext` continue to work without
//! source changes.
//

use pingora_load_balancing::Backend;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Initial EWMA value for new backends (1 ms in µs).
/// Mirrors the legacy `runtime_state::INITIAL_EWMA_US` so backends bootstrap
/// at the same latency baseline.
pub const INITIAL_EWMA_US: u64 = 1_000;

#[derive(Debug)]
pub struct EdgionBackend {
    pub(crate) inner: Backend,
    conn_count: AtomicUsize,
    ewma_us: AtomicU64,
}

impl EdgionBackend {
    #[inline]
    pub fn new(inner: Backend) -> Self {
        Self {
            inner,
            conn_count: AtomicUsize::new(0),
            ewma_us: AtomicU64::new(INITIAL_EWMA_US),
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
    /// global EWMA config. Called at request end by `lb_runtime_cleanup`.
    pub fn update_ewma(&self, latency_us: u64) {
        let alpha = crate::core::gateway::lb::ewma::get_alpha() as u64;
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
}

impl Deref for EdgionBackend {
    type Target = Backend;

    #[inline]
    fn deref(&self) -> &Backend {
        &self.inner
    }
}
