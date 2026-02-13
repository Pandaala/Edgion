//! Webhook runtime state types.
//!
//! Contains WebhookRuntime (per-webhook state), SlidingWindowCounter (rate limiter),
//! and time utilities.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use crate::types::resources::link_sys::webhook::WebhookServiceConfig;

// ============================================================
// WebhookRuntime — per-webhook runtime state
// ============================================================

/// Runtime state for a single webhook service
pub struct WebhookRuntime {
    /// Configuration from LinkSys resource
    pub config: WebhookServiceConfig,

    // ---- Health state ----
    /// Current health status (true = healthy). Lock-free reads via AtomicBool.
    pub healthy: AtomicBool,
    /// Consecutive passive failure counter (reset on success)
    pub passive_failures: AtomicU32,
    /// Timestamp of last backoff half-open attempt (for passive-only recovery)
    pub last_halfopen: AtomicU64,
    /// Current backoff interval in seconds (for passive-only recovery)
    pub backoff_sec: AtomicU64,

    // ---- Rate limit state ----
    /// Sliding window counter for outbound call rate limiting
    pub rate_counter: Option<SlidingWindowCounter>,
}

// ============================================================
// SlidingWindowCounter — simple rate limiter for webhook outbound calls
// ============================================================

/// Simple sliding window rate limiter for webhook outbound calls.
///
/// Unlike the RateLimit plugin which uses Count-Min Sketch (CMS) for
/// high-cardinality key spaces, webhook rate limiting has exactly ONE key
/// per webhook service. A simple atomic sliding window is more efficient
/// and precise for this use case.
pub struct SlidingWindowCounter {
    /// Max allowed calls in the window
    limit: u64,
    /// Window duration in seconds
    window_secs: u64,
    /// Current window counter
    current: AtomicU64,
    /// Previous window counter (for sliding interpolation)
    previous: AtomicU64,
    /// Current window start timestamp (epoch seconds)
    window_start: AtomicU64,
}

impl SlidingWindowCounter {
    pub fn new(limit: u64, window: Duration) -> Self {
        Self {
            limit,
            window_secs: window.as_secs().max(1),
            current: AtomicU64::new(0),
            previous: AtomicU64::new(0),
            window_start: AtomicU64::new(now_epoch_secs()),
        }
    }

    /// Try to acquire a permit. Returns true if allowed, false if rate limited.
    ///
    /// Uses sliding window interpolation:
    /// estimated_count = previous * (1 - elapsed_ratio) + current
    pub fn try_acquire(&self) -> bool {
        let now = now_epoch_secs();
        let ws = self.window_start.load(Ordering::Relaxed);

        // Check if we've moved to a new window
        if now >= ws + self.window_secs {
            // Rotate: current → previous, reset current
            let current = self.current.swap(0, Ordering::Relaxed);
            self.previous.store(current, Ordering::Relaxed);
            self.window_start.store(now, Ordering::Relaxed);
        }

        // Sliding window interpolation
        let elapsed = now.saturating_sub(self.window_start.load(Ordering::Relaxed));
        let ratio = (elapsed as f64) / (self.window_secs as f64);
        let prev = self.previous.load(Ordering::Relaxed) as f64;
        let curr = self.current.load(Ordering::Relaxed) as f64;
        let estimated = prev * (1.0 - ratio) + curr;

        if estimated >= self.limit as f64 {
            return false; // Rate limited
        }

        self.current.fetch_add(1, Ordering::Relaxed);
        true
    }
}

pub fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sliding_window_counter_allows_within_limit() {
        let counter = SlidingWindowCounter::new(10, Duration::from_secs(1));
        for _ in 0..10 {
            assert!(counter.try_acquire());
        }
    }

    #[test]
    fn test_sliding_window_counter_rejects_over_limit() {
        let counter = SlidingWindowCounter::new(5, Duration::from_secs(1));
        for _ in 0..5 {
            assert!(counter.try_acquire());
        }
        // 6th request should be rejected
        assert!(!counter.try_acquire());
    }

    #[test]
    fn test_sliding_window_counter_zero_limit() {
        let counter = SlidingWindowCounter::new(0, Duration::from_secs(1));
        assert!(!counter.try_acquire());
    }
}
