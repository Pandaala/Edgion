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
mod tests {
    use super::*;
    use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum Event {
        BeforeCas,
        AfterCas,
    }

    fn state(counter: &SlidingWindowCounter) -> WindowState {
        WindowState::unpack(counter.state.load(Ordering::Acquire))
    }

    // Pause exactly one public operation; other callers use the same real CAS path.
    fn interleave(
        mut counter: SlidingWindowCounter,
        now: u64,
        event: Event,
        concurrent: impl FnOnce(&SlidingWindowCounter),
    ) -> (bool, WindowState, usize) {
        use parking_lot::Mutex;
        use std::sync::{atomic::AtomicBool, mpsc};
        let (paused_tx, paused_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let resume_rx = Mutex::new(resume_rx);
        let fired = AtomicBool::new(false);
        counter.hook = Some(Arc::new(move |at| {
            if at == event && !fired.swap(true, Ordering::SeqCst) {
                paused_tx.send(()).unwrap();
                resume_rx.lock().recv_timeout(Duration::from_secs(10)).unwrap();
            }
        }));
        let samples = Arc::new(AtomicUsize::new(0));
        let reads = samples.clone();
        counter.clock = Some(Arc::new(move || {
            reads.fetch_add(1, Ordering::Relaxed);
            now
        }));
        let counter = Arc::new(counter);
        let worker_counter = counter.clone();
        let worker = std::thread::spawn(move || worker_counter.try_acquire());
        paused_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        concurrent(&counter);
        resume_tx.send(()).unwrap();
        let admitted = worker.join().unwrap();
        (admitted, state(&counter), samples.load(Ordering::Relaxed))
    }

    #[test]
    fn rotation_and_admission_publish_together() {
        // An old-window count must be retained exactly once. The paused new-window
        // admission cannot later move the concurrent admission into previous.
        for event in [Event::BeforeCas, Event::AfterCas] {
            let counter = SlidingWindowCounter::new(3, Duration::from_secs(10));
            assert!(counter.try_acquire_at(0));
            let (admitted, final_state, samples) = interleave(counter, 10, event, |c| {
                assert!(c.try_acquire_at(10));
            });
            assert!(admitted);
            assert_eq!(
                final_state,
                WindowState {
                    epoch: 1,
                    previous: 1,
                    current: 2
                }
            );
            assert_eq!(samples, 1, "CAS retries must not resample time");
        }
    }

    #[test]
    fn stale_count_cas_cannot_survive_epoch_aba() {
        let counter = SlidingWindowCounter::new(3, Duration::from_secs(10));
        assert!(counter.try_acquire_at(0));
        let (admitted, final_state, _) = interleave(counter, 0, Event::BeforeCas, |c| {
            assert!(c.try_acquire_at(20));
            assert_eq!(state(c).current, 1, "same count, different epoch");
        });
        assert!(!admitted);
        assert_eq!(
            final_state,
            WindowState {
                epoch: 2,
                previous: 0,
                current: 1
            }
        );
    }

    #[test]
    fn same_epoch_cas_retry_rechecks_admission() {
        let counter = SlidingWindowCounter::new(1, Duration::from_secs(10));
        let (admitted, final_state, samples) = interleave(counter, 0, Event::BeforeCas, |c| {
            assert!(c.try_acquire_at(0));
        });
        assert!(!admitted);
        assert_eq!(final_state.current, 1);
        assert_eq!(samples, 1);
    }

    #[test]
    fn interpolation_and_idle_expiry_use_fixed_grid() {
        let counter = SlidingWindowCounter::new(4, Duration::from_secs(10));
        for _ in 0..4 {
            assert!(counter.try_acquire_at(0));
        }
        assert!(!counter.try_acquire_at(10));
        // A late first caller does not shift the epoch origin to second 15.
        assert!(counter.try_acquire_at(15));
        assert!(counter.try_acquire_at(15));
        assert!(!counter.try_acquire_at(15));
        assert_eq!(
            state(&counter),
            WindowState {
                epoch: 1,
                previous: 4,
                current: 2
            }
        );
        for _ in 0..4 {
            assert!(counter.try_acquire_at(100));
        }
        assert!(!counter.try_acquire_at(100));
        assert_eq!(state(&counter).previous, 0);
    }

    #[test]
    fn exactly_two_idle_windows_drop_all_history() {
        let counter = SlidingWindowCounter::new(1, Duration::from_secs(10));
        assert!(counter.try_acquire_at(0));
        assert!(!counter.try_acquire_at(10));
        assert!(counter.try_acquire_at(20));
        assert_eq!(
            state(&counter),
            WindowState {
                epoch: 2,
                previous: 0,
                current: 1
            }
        );
        assert!(!counter.try_acquire_at(0), "a reversed sample cannot revive epoch zero");
    }

    #[test]
    fn representation_bounds_deny_without_wrap_and_recover_after_rotation() {
        let counter = SlidingWindowCounter::new(u64::MAX, Duration::from_secs(1));
        counter.state.store(
            WindowState {
                epoch: 0,
                previous: 0,
                current: COUNT_MAX - 1,
            }
            .pack(),
            Ordering::Release,
        );
        assert!(counter.try_acquire_at(0));
        assert!(!counter.try_acquire_at(0));
        assert_eq!(state(&counter).current, COUNT_MAX);
        assert!(counter.try_acquire_at(1));
        assert_eq!(
            state(&counter),
            WindowState {
                epoch: 1,
                previous: COUNT_MAX,
                current: 1
            }
        );
        assert!(counter.try_acquire_at(EPOCH_MAX));
        assert!(!counter.try_acquire_at(EPOCH_MAX + 1));
        assert!(!counter.try_acquire_at(u64::MAX));
        assert_eq!(
            state(&counter),
            WindowState {
                epoch: EPOCH_MAX,
                previous: 0,
                current: 1
            }
        );
    }

    #[test]
    fn whole_second_floor_and_large_duration_arithmetic() {
        let counter = SlidingWindowCounter::new(1, Duration::ZERO);
        assert!(counter.try_acquire_at(0));
        assert!(!counter.try_acquire_at(1));
        assert!(counter.try_acquire_at(2));
        let counter = SlidingWindowCounter::new(u64::MAX, Duration::from_secs(u64::MAX));
        counter.state.store(
            WindowState {
                epoch: 0,
                previous: COUNT_MAX,
                current: COUNT_MAX - 1,
            }
            .pack(),
            Ordering::Release,
        );
        assert!(counter.try_acquire_at(0));
        assert!(!counter.try_acquire_at(0));
        assert!(counter.try_acquire_at(u64::MAX));
    }

    #[test]
    fn test_sliding_window_counter_allows_within_limit() {
        let counter = SlidingWindowCounter::new(10, Duration::from_secs(1));
        for _ in 0..10 {
            assert!(counter.try_acquire_at(0));
        }
    }

    #[test]
    fn test_sliding_window_counter_rejects_over_limit() {
        let counter = SlidingWindowCounter::new(5, Duration::from_secs(1));
        for _ in 0..5 {
            assert!(counter.try_acquire_at(0));
        }
        // 6th request should be rejected
        assert!(!counter.try_acquire_at(0));
    }

    #[test]
    fn test_sliding_window_counter_zero_limit() {
        let counter = SlidingWindowCounter::new(0, Duration::from_secs(1));
        assert!(!counter.try_acquire_at(0));
    }

    #[test]
    fn test_sliding_window_counter_concurrent_does_not_over_admit() {
        use std::thread;

        // A fixed sample makes this independent of scheduling delays.
        let limit = 1000u64;
        let counter = Arc::new(SlidingWindowCounter::new(limit, Duration::from_secs(60)));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let c = Arc::clone(&counter);
                thread::spawn(move || {
                    let mut granted = 0u64;
                    for _ in 0..2000 {
                        if c.try_acquire_at(0) {
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
    fn concurrent_rotation_preserves_weighted_quota() {
        let counter = Arc::new(SlidingWindowCounter::new(1000, Duration::from_secs(10)));
        for _ in 0..1000 {
            assert!(counter.try_acquire_at(0));
        }
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let counter = counter.clone();
                std::thread::spawn(move || (0..1000).filter(|_| counter.try_acquire_at(15)).count())
            })
            .collect();
        let admitted: usize = workers.into_iter().map(|worker| worker.join().unwrap()).sum();
        assert_eq!(admitted, 500);
        assert_eq!(
            state(&counter),
            WindowState {
                epoch: 1,
                previous: 1000,
                current: 500
            }
        );
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
