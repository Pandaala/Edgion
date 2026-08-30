//! Webhook runtime state types.
//!
//! Contains SlidingWindowCounter (rate limiter) and time utilities.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// Re-export from the shared outbound engine.
pub use crate::plugins::http::common::outbound::target::rr_pick;

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
    use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

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

    #[test]
    fn rr_pick_rotates_and_handles_empty() {
        use std::sync::atomic::AtomicUsize;
        let cursor = AtomicUsize::new(0);
        let eps = vec!["10.0.0.1:8080".to_string(), "[2001:db8::1]:8080".to_string()];

        let a = super::rr_pick(&eps, &cursor, "http");
        let b = super::rr_pick(&eps, &cursor, "https");
        let c = super::rr_pick(&eps, &cursor, "http");
        assert_eq!(a.as_deref(), Some("http://10.0.0.1:8080"));
        assert_eq!(b.as_deref(), Some("https://[2001:db8::1]:8080"));
        assert_eq!(c.as_deref(), Some("http://10.0.0.1:8080")); // wrapped around

        let empty: Vec<String> = vec![];
        assert_eq!(super::rr_pick(&empty, &cursor, "http"), None);
    }

    /// `ResolvedTarget::Service` resolves from the global EpSliceStore.
    ///
    /// Verifies that `pick_url()` returns `None` when the store has no entry for the
    /// service key, and `Some(url)` after an endpoint slice is seeded via the
    /// incremental `update_data_only` path (unique slice key — no cross-test pollution).
    #[test]
    fn pick_url_service_resolves_from_global_store() {
        use crate::plugins::http::common::outbound::target::ResolvedTarget;
        use edgion_resources::constants::labels::k8s::SERVICE_NAME;
        use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
        use std::collections::{BTreeMap, HashMap, HashSet};

        let svc_key = "wh-pickurl-test/svc";
        let slice_key = "wh-pickurl-test/svc-slice";

        let target = ResolvedTarget::Service {
            service_key: svc_key.to_string(),
            port: Some(8080),
            scheme: "http",
            cursor: AtomicUsize::new(0),
        };

        // No entry in the global store yet: pick_url must return None.
        assert_eq!(target.pick_url(), None);

        // Build a ready EndpointSlice whose label resolves to "wh-pickurl-test/svc".
        let mut labels = BTreeMap::new();
        labels.insert(SERVICE_NAME.to_string(), "svc".to_string());
        let ep_slice = EndpointSlice {
            metadata: ObjectMeta {
                name: Some("svc-slice".to_string()),
                namespace: Some("wh-pickurl-test".to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            address_type: "IPv4".to_string(),
            endpoints: vec![Endpoint {
                addresses: vec!["10.0.0.1".to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ports: Some(vec![EndpointPort {
                port: Some(8080),
                ..Default::default()
            }]),
        };

        // Incremental add: only this unique slice key is touched; other test keys unaffected.
        let add = HashMap::from([(slice_key.to_string(), ep_slice)]);
        get_roundrobin_store().update_data_only(add, HashMap::new(), &HashSet::new());

        // After seeding: pick_url resolves the ready endpoint.
        assert_eq!(target.pick_url(), Some("http://10.0.0.1:8080".to_string()));

        // Cleanup: remove so we do not leave stale entries in the global store.
        let remove = HashSet::from([slice_key.to_string()]);
        get_roundrobin_store().update_data_only(HashMap::new(), HashMap::new(), &remove);
    }
}
