//! Webhook runtime state types.
//!
//! Contains WebhookRuntime (per-webhook state), SlidingWindowCounter (rate limiter),
//! and time utilities.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::lb::WeightedRoundRobin;
use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;
use reqwest::Client;

// ============================================================
// WebhookResolvedTarget — resolved runtime target
// ============================================================

/// Resolved target for a webhook service at runtime.
///
/// Derived from `WebhookTarget` during manager upsert. Decouples
/// routing decisions from the raw configuration.
pub enum WebhookResolvedTarget {
    /// Single base URL: Uri target or ServiceClusterIp.
    Static(String),
    /// WRR pool of pod IPs: Service/ServiceEndpointSlice with injected endpoints.
    Endpoints(WeightedRoundRobin<String>),
    /// Endpoints not yet injected by Controller (transient state).
    Pending,
}

impl std::fmt::Debug for WebhookResolvedTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(url) => write!(f, "Static({url:?})"),
            Self::Endpoints(rr) => write!(f, "Endpoints(count={})", rr.len()),
            Self::Pending => write!(f, "Pending"),
        }
    }
}

impl WebhookResolvedTarget {
    /// Pick the next base URL. Returns `None` when the target is `Pending`.
    pub fn pick_url(&self) -> Option<String> {
        match self {
            Self::Static(url) => Some(url.clone()),
            Self::Endpoints(rr) => Some(rr.select().clone()),
            Self::Pending => None,
        }
    }
}

// ============================================================
// WebhookRuntime — per-webhook runtime state
// ============================================================

/// Runtime state for a single webhook service
pub struct WebhookRuntime {
    /// Configuration from LinkSys resource
    pub config: WebhookServiceConfig,

    /// HTTP client for this webhook. When TLS config is present, this is a custom client.
    pub http_client: Arc<Client>,

    /// Resolved target for outbound requests (derived from `config.target` at upsert time).
    pub resolved_target: WebhookResolvedTarget,

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
        let ws = self.window_start.load(Ordering::Acquire);

        if now >= ws + self.window_secs {
            // CAS ensures only one thread performs the rotation
            if self
                .window_start
                .compare_exchange(ws, now, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                let current = self.current.swap(0, Ordering::AcqRel);
                self.previous.store(current, Ordering::Release);
            }
        }

        let ws_after = self.window_start.load(Ordering::Acquire);
        let elapsed = now.saturating_sub(ws_after);
        let ratio = (elapsed as f64) / (self.window_secs as f64);
        let prev = self.previous.load(Ordering::Acquire) as f64;

        // Decide and increment atomically. A plain check-then-fetch_add lets
        // concurrent callers all observe the same sub-limit `estimated`, all pass
        // the check, and all increment, admitting more than `limit` per window.
        // The CAS retry loop only commits the increment when `current` is unchanged
        // since the read, so concurrent callers serialize on the counter and the
        // limit holds exactly.
        loop {
            let curr = self.current.load(Ordering::Acquire);
            let estimated = prev * (1.0 - ratio) + curr as f64;
            if estimated >= self.limit as f64 {
                return false;
            }
            if self
                .current
                .compare_exchange_weak(curr, curr + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
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

    #[test]
    fn test_sliding_window_counter_concurrent_does_not_over_admit() {
        use std::thread;

        // Large window so no rotation happens during the test: `previous` stays 0
        // and `ratio` ~= 0, so `estimated == current` and exactly `limit` permits
        // exist. With a non-atomic check-then-fetch_add, concurrent threads could
        // all pass the check at the boundary and over-admit; the CAS retry loop
        // must grant exactly `limit`.
        let limit = 1000u64;
        let counter = Arc::new(SlidingWindowCounter::new(limit, Duration::from_secs(60)));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let c = Arc::clone(&counter);
                thread::spawn(move || {
                    let mut granted = 0u64;
                    for _ in 0..2000 {
                        if c.try_acquire() {
                            granted += 1;
                        }
                    }
                    granted
                })
            })
            .collect();

        let total: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(total, limit);
    }
}
