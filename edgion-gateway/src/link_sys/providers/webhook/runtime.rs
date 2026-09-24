//! Webhook runtime state types.
//!
//! Contains the shared outbound sliding-window rate limiter.

use portable_atomic::AtomicU128;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

// Re-export from the shared outbound engine.
pub use crate::plugins::http::common::outbound::target::rr_pick;

// ============================================================
// SlidingWindowCounter — simple rate limiter for webhook outbound calls
// ============================================================

const COUNT_BITS: u32 = 40;
const COUNT_MAX: u64 = (1 << COUNT_BITS) - 1;
const EPOCH_MAX: u64 = (1 << 48) - 1;

/// One coherent window: 48 epoch bits and two 40-bit counts. Neither epochs
/// nor counts wrap, so an equal packed word cannot hide a rotation (ABA).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WindowState {
    epoch: u64,
    previous: u64,
    current: u64,
}

impl WindowState {
    fn unpack(word: u128) -> Self {
        Self {
            epoch: (word >> (2 * COUNT_BITS)) as u64,
            previous: ((word >> COUNT_BITS) as u64) & COUNT_MAX,
            current: (word as u64) & COUNT_MAX,
        }
    }

    fn pack(self) -> u128 {
        ((self.epoch as u128) << (2 * COUNT_BITS)) | ((self.previous as u128) << COUNT_BITS) | self.current as u128
    }
}

/// Two-bucket sliding interpolation, shared by outbound HTTP clients.
/// Rotation, admission and increment commit in one CAS. Gateway startup
/// requires native 128-bit atomics; the serving path allocates nothing and
/// takes no locks. CAS retries are lock-free, not wait-free.
pub struct SlidingWindowCounter {
    /// Max allowed calls in the window
    limit: u64,
    /// Window duration in seconds
    window_secs: u64,
    /// Monotonic origin for fixed-grid, whole-second windows.
    origin: Instant,
    /// Epoch, previous count and current count, published together.
    state: AtomicU128,
    #[cfg(test)]
    clock: Option<std::sync::Arc<dyn Fn() -> u64 + Send + Sync>>,
    #[cfg(test)]
    hook: Option<std::sync::Arc<dyn Fn(tests::Event) + Send + Sync>>,
}

impl SlidingWindowCounter {
    pub fn new(limit: u64, window: Duration) -> Self {
        Self {
            limit,
            window_secs: window.as_secs().max(1),
            origin: Instant::now(),
            state: AtomicU128::new(0),
            #[cfg(test)]
            clock: None,
            #[cfg(test)]
            hook: None,
        }
    }

    /// Try to acquire a permit. Returns true if allowed, false if rate limited.
    ///
    /// Uses sliding window interpolation:
    /// estimated_count = previous * (1 - elapsed_ratio) + current
    pub fn try_acquire(&self) -> bool {
        self.try_acquire_at(self.now_secs())
    }

    fn now_secs(&self) -> u64 {
        #[cfg(test)]
        if let Some(clock) = &self.clock {
            return clock();
        }
        Instant::now().saturating_duration_since(self.origin).as_secs()
    }

    fn try_acquire_at(&self, now_secs: u64) -> bool {
        let epoch = now_secs / self.window_secs;
        if epoch > EPOCH_MAX {
            return false;
        }
        let remaining = self.window_secs - now_secs % self.window_secs;
        let mut old = self.state.load(Ordering::Acquire);
        loop {
            let mut next = WindowState::unpack(old);
            if next.epoch > epoch {
                // A delayed decision may not resurrect an already replaced epoch.
                return false;
            }
            if next.epoch < epoch {
                next.previous = if epoch - next.epoch == 1 { next.current } else { 0 };
                next.current = 0;
                next.epoch = epoch;
            }
            // Exact integer form of previous * remaining/window + current < limit.
            // Counts are 40-bit and durations/limits 64-bit: both sides fit u128.
            let weighted = next.previous as u128 * remaining as u128 + next.current as u128 * self.window_secs as u128;
            if next.current == COUNT_MAX || weighted >= self.limit as u128 * self.window_secs as u128 {
                return false;
            }
            next.current += 1;
            #[cfg(test)]
            self.notify(tests::Event::BeforeCas);
            match self
                .state
                .compare_exchange_weak(old, next.pack(), Ordering::AcqRel, Ordering::Acquire)
            {
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

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/webhook/runtime_test.rs"]
mod tests;
