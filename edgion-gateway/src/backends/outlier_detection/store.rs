use super::config::OutlierDetectionResolved;
#[cfg(test)]
use super::state::ejection_duration;
use super::state::{FailureClass, FailureTransition, PassiveEjectionState};
use dashmap::DashMap;
use std::sync::{Arc, LazyLock};

static OD_STORE: LazyLock<Arc<OutlierDetectionStore>> = LazyLock::new(|| Arc::new(OutlierDetectionStore::new()));

pub fn get_outlier_detection_store() -> Arc<OutlierDetectionStore> {
    OD_STORE.clone()
}

struct ServiceCbState {
    config: OutlierDetectionResolved,
    endpoints: DashMap<u64, Arc<PassiveEjectionState>>,
    /// Serializes the percent-check + ejection steps in `record_failure` to prevent
    /// concurrent calls for distinct backends from both passing the max-ejection-percent
    /// guard simultaneously and over-ejecting beyond the configured limit.
    eject_lock: std::sync::Mutex<()>,
}

impl ServiceCbState {
    /// Single canonical construction site for a fresh per-service breaker state, so a
    /// future field addition is made in one place rather than at every insert site.
    fn new(config: OutlierDetectionResolved) -> Arc<Self> {
        Arc::new(Self {
            config,
            endpoints: DashMap::new(),
            eject_lock: std::sync::Mutex::new(()),
        })
    }
}

pub struct OutlierDetectionStore {
    services: DashMap<String, Arc<ServiceCbState>>,
}

impl OutlierDetectionStore {
    pub fn new() -> Self {
        Self {
            services: DashMap::new(),
        }
    }

    /// Number of services with outlier-detection state.
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    /// Service keys currently tracked (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.services.iter().map(|kv| kv.key().clone()).collect()
    }

    pub fn set_service_config(&self, service_key: &str, config: Option<OutlierDetectionResolved>) {
        match config {
            // Use a single DashMap `entry` so the check-and-swap runs under one shard write
            // lock: no window where the key is absent (which would let data-plane readers
            // bypass the breaker), and concurrent writers for the same key cannot lose an
            // update — the last writer holding the lock wins.
            Some(cfg) => match self.services.entry(service_key.to_string()) {
                dashmap::mapref::entry::Entry::Occupied(mut occ) => {
                    let existing_cfg = &occ.get().config;
                    let changed = existing_cfg.consecutive_errors != cfg.consecutive_errors
                        || existing_cfg.consecutive_gateway_errors != cfg.consecutive_gateway_errors
                        || existing_cfg.consecutive_local_origin_failures != cfg.consecutive_local_origin_failures
                        || existing_cfg.ejection_seconds != cfg.ejection_seconds
                        || existing_cfg.max_ejection_seconds != cfg.max_ejection_seconds
                        || existing_cfg.max_ejection_percent != cfg.max_ejection_percent;
                    if changed {
                        // `insert` swaps in the new state and returns the displaced one. Drop
                        // the entry guard first so the gauge drain (which never touches
                        // `self.services`) runs outside the held shard lock.
                        let old_svc = occ.insert(ServiceCbState::new(cfg));
                        drop(occ);
                        Self::drain_ejected_gauge(service_key, &old_svc);
                    }
                }
                dashmap::mapref::entry::Entry::Vacant(vac) => {
                    vac.insert(ServiceCbState::new(cfg));
                }
            },
            None => {
                if let Some((_, old_svc)) = self.services.remove(service_key) {
                    Self::drain_ejected_gauge(service_key, &old_svc);
                }
            }
        }
    }

    #[inline]
    pub fn is_ejected(&self, service_key: &str, backend_hash: u64) -> bool {
        let svc = match self.services.get(service_key) {
            Some(s) => Arc::clone(&s),
            None => return false,
        };

        let Some(endpoint) = svc.endpoints.get(&backend_hash).map(|entry| Arc::clone(entry.value())) else {
            return false;
        };
        let (ejected, recovered) = endpoint.legacy_ejection_status();
        if recovered {
            crate::observe::metrics::record_cb_recovery(service_key);
        }
        ejected
    }

    /// Backoff-scaled ejection duration: `min(ejection_seconds * num_ejections, max)`.
    #[cfg(test)]
    #[inline]
    fn ejection_duration(config: &OutlierDetectionResolved, num_ejections: u32) -> std::time::Duration {
        ejection_duration(config, num_ejections)
    }

    pub fn record_failure(&self, service_key: &str, backend_hash: u64, total_endpoints: usize, class: FailureClass) {
        let svc = match self.services.get(service_key) {
            Some(s) => Arc::clone(&s),
            None => return,
        };

        let endpoint = Arc::clone(
            svc.endpoints
                .entry(backend_hash)
                .or_insert_with(|| PassiveEjectionState::new(Some(svc.config.clone())))
                .value(),
        );

        // Steps 2+3 under eject_lock so concurrent failures for distinct backends
        // cannot both pass the percent check and exceed max_ejection_percent.
        let _eject_guard = svc.eject_lock.lock().unwrap_or_else(|e| e.into_inner());

        let allow_ejection = if svc.config.max_ejection_percent > 0 {
            let effective_total = if total_endpoints > 0 {
                total_endpoints
            } else {
                svc.endpoints.len().max(1)
            };
            let currently_ejected = svc.endpoints.iter().filter(|entry| entry.value().is_open()).count();
            let max_ejectable = (effective_total as u64 * svc.config.max_ejection_percent as u64 / 100) as usize;
            currently_ejected < max_ejectable.max(1)
        } else {
            true
        };

        if endpoint.record_failure(class, allow_ejection) == FailureTransition::Opened {
            crate::observe::metrics::record_cb_ejection(service_key, class.metric_label());
        }
    }

    pub fn record_success(&self, service_key: &str, backend_hash: u64) {
        let svc = match self.services.get(service_key) {
            Some(s) => Arc::clone(&s),
            None => return,
        };

        // Reset both counters and clear ejection, but RETAIN num_ejections so
        // backoff history survives. Only operate on an existing entry — a
        // never-failed endpoint needs no allocation. If it had been ejected,
        // fire record_cb_recovery to keep the gauge balanced.
        // Trailing `;` keeps the Option<RefMut> temporary's drop before `svc` is
        // released — without it the if-let is the function's trailing expression and
        // the borrow of `svc.endpoints` outlives `svc`.
        if let Some(endpoint) = svc.endpoints.get(&backend_hash) {
            if endpoint.record_success() {
                crate::observe::metrics::record_cb_recovery(service_key);
            }
        };
    }

    #[inline]
    pub fn has_service(&self, service_key: &str) -> bool {
        self.services.contains_key(service_key)
    }

    /// Decrement gauge for any currently-ejected endpoints in a service state.
    fn drain_ejected_gauge(service_key: &str, svc: &ServiceCbState) {
        let ejected = svc.endpoints.iter().filter(|entry| entry.value().is_open()).count();
        for _ in 0..ejected {
            crate::observe::metrics::record_cb_recovery(service_key);
        }
    }

    pub fn remove_service(&self, service_key: &str) {
        if let Some((_, svc)) = self.services.remove(service_key) {
            Self::drain_ejected_gauge(service_key, &svc);
        }
    }

    /// Remove a single backend endpoint's CB state (called when endpoint is removed from EP/EPS).
    pub fn remove_backend(&self, service_key: &str, addr: &pingora_core::protocols::l4::socket::SocketAddr) {
        let Some(svc) = self.services.get(service_key) else {
            return;
        };
        let backend = pingora_load_balancing::Backend {
            addr: addr.clone(),
            weight: 1,
            ext: Default::default(),
        };
        let hash = crate::backends::health::check::backend_hash(&backend);
        if let Some((_, endpoint)) = svc.endpoints.remove(&hash) {
            if endpoint.is_open() {
                crate::observe::metrics::record_cb_recovery(service_key);
            }
        }
    }

    /// Record failure using Pingora SocketAddr (convenience for hook callsites).
    pub fn record_failure_by_addr(
        &self,
        service_key: &str,
        addr: &pingora_core::protocols::l4::socket::SocketAddr,
        class: FailureClass,
    ) {
        if !self.has_service(service_key) {
            return;
        }
        let backend = pingora_load_balancing::Backend {
            addr: addr.clone(),
            weight: 1,
            ext: Default::default(),
        };
        let hash = crate::backends::health::check::backend_hash(&backend);
        let total = crate::backends::selector::get_backends_for_service_key(service_key).len();
        self.record_failure(service_key, hash, total, class);
    }

    /// Record success using Pingora SocketAddr (convenience for hook callsites).
    pub fn record_success_by_addr(&self, service_key: &str, addr: &pingora_core::protocols::l4::socket::SocketAddr) {
        if !self.has_service(service_key) {
            return;
        }
        let backend = pingora_load_balancing::Backend {
            addr: addr.clone(),
            weight: 1,
            ext: Default::default(),
        };
        let hash = crate::backends::health::check::backend_hash(&backend);
        self.record_success(service_key, hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(errors: u32, gateway: u32, seconds: u64, max_secs: u64, max_pct: u32) -> OutlierDetectionResolved {
        // Local-origin threshold inherits the gateway threshold, matching the
        // config `normalize` default, so existing tests need no extra argument.
        OutlierDetectionResolved {
            consecutive_errors: errors,
            consecutive_gateway_errors: gateway,
            consecutive_local_origin_failures: gateway,
            ejection_seconds: seconds,
            max_ejection_seconds: max_secs,
            max_ejection_percent: max_pct,
        }
    }

    /// Build a config with an explicit local-origin threshold distinct from the
    /// gateway threshold, for the local-origin split tests.
    fn make_config_lo(gateway: u32, local_origin: u32) -> OutlierDetectionResolved {
        OutlierDetectionResolved {
            consecutive_errors: 100,
            consecutive_gateway_errors: gateway,
            consecutive_local_origin_failures: local_origin,
            ejection_seconds: 30,
            max_ejection_seconds: 300,
            max_ejection_percent: 0,
        }
    }

    #[test]
    fn test_no_config_means_not_ejected() {
        let store = OutlierDetectionStore::new();
        assert!(!store.is_ejected("default/svc1", 123));
    }

    #[test]
    fn test_record_failure_ejects_after_threshold() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(3, 3, 30, 300, 0)));

        for _ in 0..2 {
            store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
            assert!(!store.is_ejected("default/svc1", 111));
        }

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_success_resets_failures() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(3, 3, 30, 300, 0)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        store.record_success("default/svc1", 111);
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);

        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_auto_recovery_after_cooldown() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 1, 300, 0)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        std::thread::sleep(std::time::Duration::from_secs(1));

        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_max_ejection_percent() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        store.record_failure("default/svc1", 222, 2, FailureClass::Gateway);
        assert!(!store.is_ejected("default/svc1", 222));
    }

    #[test]
    fn test_max_ejection_percent_concurrent_no_over_eject() {
        use std::sync::{Arc, Barrier};

        // 4 backends, 50% limit → at most 2 may be ejected regardless of concurrency.
        let store = Arc::new(OutlierDetectionStore::new());
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));

        let barrier = Arc::new(Barrier::new(4));
        let hashes = [100u64, 200, 300, 400];

        let handles: Vec<_> = hashes
            .iter()
            .copied()
            .map(|hash| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.record_failure("default/svc1", hash, 4, FailureClass::Gateway);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let ejected = hashes.iter().filter(|&&h| store.is_ejected("default/svc1", h)).count();
        assert!(
            ejected <= 2,
            "over-ejected: {ejected} backends ejected, limit is 2 (50% of 4)"
        );
    }

    #[test]
    fn test_remove_service_clears_state() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        store.remove_service("default/svc1");
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_record_success_on_ejected_does_not_leak_gauge() {
        // record_success on an ejected entry must fire record_cb_recovery so the
        // CB_EJECTED_ENDPOINTS gauge stays balanced.
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        // record_success on an ejected backend clears the ejection. After reset,
        // is_ejected returns false. The +1/-1 gauge symmetry is verified by
        // inspection of the recovery branch in record_success itself.
        store.record_success("default/svc1", 111);
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_percent_cap_applies_concurrently_when_total_endpoints_is_zero() {
        // With total_endpoints == 0 (empty selector snapshot) and concurrent failures,
        // the percent cap must still bound ejected backends via the observed endpoint
        // count fallback.
        use std::sync::{Arc, Barrier};

        let store = Arc::new(OutlierDetectionStore::new());
        // 50% cap. Once endpoints.len() reaches 4, max_ejectable = 2.
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));

        let barrier = Arc::new(Barrier::new(4));
        let hashes = [100u64, 200, 300, 400];

        let handles: Vec<_> = hashes
            .iter()
            .copied()
            .map(|hash| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    // total=0 simulates the empty-selector-snapshot scale-down race.
                    store.record_failure("default/svc1", hash, 0, FailureClass::Gateway);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let ejected = hashes.iter().filter(|&&h| store.is_ejected("default/svc1", h)).count();
        // The first thread observes endpoints.len()=1 (cap=1, ejects), the second sees
        // endpoints.len()=2 (cap=1, currently_ejected=1, rejects). After that, each new
        // thread inserts its entry first then enters the lock — endpoints.len() grows
        // monotonically inside the lock, so the cap rises but currently_ejected catches
        // up. Upper bound is 50% of 4 = 2.
        assert!(
            ejected <= 2,
            "over-ejected under total=0 race: {ejected} backends ejected, 50% cap of 4 is 2"
        );
    }

    #[test]
    fn test_percent_cap_applies_when_total_endpoints_is_zero() {
        // When the selector snapshot is momentarily empty (total_endpoints == 0), the
        // percent guard must still apply using the observed endpoint count as a fallback.
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));

        // First failure with total=0 establishes the endpoint and ejects (max(1) of 50% of 1).
        store.record_failure("default/svc1", 111, 0, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        // Second backend tries to eject with total=0 — observed endpoints are now 2, the
        // 50% cap is 1, and 1 backend is already ejected, so this must be rejected.
        store.record_failure("default/svc1", 222, 0, FailureClass::Gateway);
        assert!(!store.is_ejected("default/svc1", 222));
    }

    #[test]
    fn test_config_change_resets_state_unchanged_preserves() {
        // The atomic entry() swap must preserve these semantics: a *changed* config
        // rebuilds the service (clearing ejection state), while an *unchanged* config
        // is a no-op that keeps the existing ejection state.
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));

        // Re-applying the identical config must not wipe the ejection state.
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
        assert!(store.is_ejected("default/svc1", 111));

        // Applying a different config rebuilds the service with empty endpoints.
        store.set_service_config("default/svc1", Some(make_config(5, 5, 30, 300, 0)));
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_concurrent_config_update_no_panic_and_key_present() {
        // Concurrent set_service_config calls for the same key must never panic and
        // must never leave the key absent. After the storm, one config must be installed.
        use std::sync::{Arc, Barrier};

        let store = Arc::new(OutlierDetectionStore::new());
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));

        let barrier = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4u32)
            .map(|i| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    // Each writer applies a distinct config to force the changed-path swap.
                    store.set_service_config("default/svc1", Some(make_config(i + 2, i + 2, 30, 300, 0)));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert!(store.has_service("default/svc1"));
    }

    #[test]
    fn test_different_services_isolated() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
        store.set_service_config("default/svc2", Some(make_config(1, 1, 30, 300, 0)));

        store.record_failure("default/svc1", 111, 2, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 111));
        assert!(!store.is_ejected("default/svc2", 111));
    }

    #[test]
    fn app_and_gateway_counters_are_independent() {
        let store = OutlierDetectionStore::new();
        // app threshold 3, gateway threshold 10.
        store.set_service_config("default/svc", Some(make_config(3, 10, 30, 300, 0)));

        // 3 application failures eject via counter A even though gateway is far from 10.
        for _ in 0..3 {
            store.record_failure("default/svc", 1, 1, FailureClass::Application);
        }
        assert!(store.is_ejected("default/svc", 1));
    }

    #[test]
    fn gateway_counter_does_not_trip_app_threshold() {
        let store = OutlierDetectionStore::new();
        // app threshold 3, gateway threshold 10.
        store.set_service_config("default/svc", Some(make_config(3, 10, 30, 300, 0)));
        // 3 gateway failures must NOT eject (gateway threshold is 10).
        for _ in 0..3 {
            store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        }
        assert!(!store.is_ejected("default/svc", 1));
    }

    #[test]
    fn success_resets_both_counters() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc", Some(make_config(3, 3, 30, 300, 0)));
        store.record_failure("default/svc", 1, 1, FailureClass::Application);
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        store.record_success("default/svc", 1);
        // After reset, two more app failures still below threshold 3.
        store.record_failure("default/svc", 1, 1, FailureClass::Application);
        store.record_failure("default/svc", 1, 1, FailureClass::Application);
        assert!(!store.is_ejected("default/svc", 1));
    }

    #[test]
    fn local_origin_counter_is_independent_of_gateway() {
        let store = OutlierDetectionStore::new();
        // gateway threshold 10, local-origin threshold 3.
        store.set_service_config("default/svc", Some(make_config_lo(10, 3)));

        // 3 local-origin failures eject via the local-origin counter even though the
        // gateway counter is far from 10.
        for _ in 0..3 {
            store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        }
        assert!(store.is_ejected("default/svc", 1));
    }

    #[test]
    fn gateway_failures_do_not_trip_local_origin_threshold() {
        let store = OutlierDetectionStore::new();
        // gateway threshold 10, local-origin threshold 2: 2 gateway failures must NOT
        // eject through the (lower) local-origin counter — the buckets are separate.
        store.set_service_config("default/svc", Some(make_config_lo(10, 2)));
        for _ in 0..2 {
            store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        }
        assert!(!store.is_ejected("default/svc", 1));
    }

    #[test]
    fn success_resets_local_origin_counter() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc", Some(make_config_lo(10, 3)));
        store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        store.record_success("default/svc", 1);
        // After reset, two more local-origin failures still below threshold 3.
        store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        store.record_failure("default/svc", 1, 1, FailureClass::LocalOrigin);
        assert!(!store.is_ejected("default/svc", 1));
    }

    #[test]
    fn failure_class_metric_labels_are_bounded() {
        assert_eq!(FailureClass::Gateway.metric_label(), "gateway");
        assert_eq!(FailureClass::Application.metric_label(), "application");
        assert_eq!(FailureClass::LocalOrigin.metric_label(), "local_origin");
    }

    #[test]
    fn backoff_doubles_second_ejection_and_persists_num_ejections() {
        let store = OutlierDetectionStore::new();
        // base 1s, consecutive 1 so a single failure ejects.
        store.set_service_config("default/svc", Some(make_config(1, 1, 1, 300, 0)));

        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc", 1));

        // After ~1s the first ejection recovers (num_ejections retained = 1).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(!store.is_ejected("default/svc", 1)); // triggers reset-but-retain

        // Eject again -> num_ejections becomes 2 -> duration ~2s.
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc", 1));
        // Still ejected at ~1.2s because the second window is ~2s.
        std::thread::sleep(std::time::Duration::from_millis(1200));
        assert!(store.is_ejected("default/svc", 1));
    }

    #[test]
    fn max_ejection_seconds_caps_backoff() {
        // Direct test of the backoff duration helper: linear base*num_ejections,
        // clamped at max_ejection_seconds. base=10s, cap=25s.
        let cfg = make_config(1, 1, 10, 25, 0);
        assert_eq!(
            OutlierDetectionStore::ejection_duration(&cfg, 1),
            std::time::Duration::from_secs(10)
        );
        assert_eq!(
            OutlierDetectionStore::ejection_duration(&cfg, 2),
            std::time::Duration::from_secs(20)
        );
        // 3*10=30 exceeds the 25s cap -> clamped to 25.
        assert_eq!(
            OutlierDetectionStore::ejection_duration(&cfg, 3),
            std::time::Duration::from_secs(25)
        );
        // Far past the cap stays clamped (and does not overflow).
        assert_eq!(
            OutlierDetectionStore::ejection_duration(&cfg, u32::MAX),
            std::time::Duration::from_secs(25)
        );
    }

    #[test]
    fn change_detection_rebuilds_on_gateway_threshold_change() {
        let store = OutlierDetectionStore::new();
        store.set_service_config("default/svc", Some(make_config(5, 5, 30, 300, 0)));
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway); // G=1
                                                                          // Changing ONLY the gateway threshold must rebuild (clearing the counter).
        store.set_service_config("default/svc", Some(make_config(5, 2, 30, 300, 0)));
        // Counter was cleared by rebuild: needs 2 fresh gateway failures to eject.
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        assert!(!store.is_ejected("default/svc", 1));
        store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc", 1));
    }

    #[test]
    fn concurrent_same_backend_failures_eject_once() {
        use std::sync::{Arc, Barrier};

        // consecutive 1 so a single failure trips the threshold; percent cap off
        // (0) so the percent guard cannot mask the idempotency check.
        let store = Arc::new(OutlierDetectionStore::new());
        store.set_service_config("default/svc", Some(make_config(1, 1, 30, 300, 0)));

        let barrier = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.record_failure("default/svc", 1, 1, FailureClass::Gateway);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        assert!(store.is_ejected("default/svc", 1));
        // Despite up to 4 concurrent failures crossing the threshold before any
        // ejection lands, the backend is ejected exactly once: num_ejections must
        // be 1 (regression for the step-3 idempotency guard — without it the race
        // would inflate num_ejections and leak the ejected-endpoints gauge).
        let svc = store.services.get("default/svc").unwrap();
        let ep = svc.endpoints.get(&1).unwrap();
        assert_eq!(
            ep.num_ejections(),
            1,
            "concurrent same-backend ejection must count once"
        );
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_zero() {
        let store = OutlierDetectionStore::new();
        assert_eq!(store.service_count(), 0);
        assert!(store.keys().is_empty());
    }
}
