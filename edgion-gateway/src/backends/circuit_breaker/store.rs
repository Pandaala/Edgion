use super::config::CircuitBreakerConfig;
use dashmap::DashMap;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

static CB_STORE: LazyLock<Arc<CircuitBreakerStore>> = LazyLock::new(|| Arc::new(CircuitBreakerStore::new()));

pub fn get_circuit_breaker_store() -> Arc<CircuitBreakerStore> {
    CB_STORE.clone()
}

#[derive(Debug)]
struct EndpointState {
    consecutive_failures: u32,
    ejected_at: Option<Instant>,
}

impl Default for EndpointState {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            ejected_at: None,
        }
    }
}

struct ServiceCbState {
    config: CircuitBreakerConfig,
    endpoints: DashMap<u64, EndpointState>,
    /// Serializes the percent-check + ejection steps in `record_failure` to prevent
    /// concurrent calls for distinct backends from both passing the max-ejection-percent
    /// guard simultaneously and over-ejecting beyond the configured limit.
    eject_lock: std::sync::Mutex<()>,
}

impl ServiceCbState {
    /// Single canonical construction site for a fresh per-service breaker state, so a
    /// future field addition is made in one place rather than at every insert site.
    fn new(config: CircuitBreakerConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            endpoints: DashMap::new(),
            eject_lock: std::sync::Mutex::new(()),
        })
    }
}

pub struct CircuitBreakerStore {
    services: DashMap<String, Arc<ServiceCbState>>,
}

impl CircuitBreakerStore {
    pub fn new() -> Self {
        Self {
            services: DashMap::new(),
        }
    }

    /// Number of services with circuit-breaker state.
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    /// Service keys currently tracked (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.services.iter().map(|kv| kv.key().clone()).collect()
    }

    pub fn set_service_config(&self, service_key: &str, config: Option<CircuitBreakerConfig>) {
        match config {
            // Use a single DashMap `entry` so the check-and-swap runs under one shard write
            // lock. This makes the config update atomic: there is no window where the key is
            // absent (the old `get`+`remove`+`insert` form briefly removed the entry, which
            // let concurrent data-plane readers bypass the breaker), and concurrent writers
            // for the same key cannot lose an update — the last writer holding the lock wins.
            Some(cfg) => match self.services.entry(service_key.to_string()) {
                dashmap::mapref::entry::Entry::Occupied(mut occ) => {
                    let existing_cfg = &occ.get().config;
                    let changed = existing_cfg.consecutive_errors != cfg.consecutive_errors
                        || existing_cfg.ejection_seconds != cfg.ejection_seconds
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

        let Some(ep) = svc.endpoints.get(&backend_hash) else {
            return false;
        };

        let Some(ejected_at) = ep.ejected_at else {
            return false;
        };

        if Instant::now().saturating_duration_since(ejected_at) >= svc.config.ejection_duration() {
            drop(ep);
            if svc.endpoints.remove(&backend_hash).is_some() {
                crate::observe::metrics::record_cb_recovery(service_key);
            }
            return false;
        }

        true
    }

    pub fn record_failure(&self, service_key: &str, backend_hash: u64, total_endpoints: usize) {
        let svc = match self.services.get(service_key) {
            Some(s) => Arc::clone(&s),
            None => return,
        };

        // Step 1: increment failure count (short-lived lock)
        let should_eject = {
            let mut ep = svc.endpoints.entry(backend_hash).or_default();
            if ep.ejected_at.is_some() {
                return;
            }
            ep.consecutive_failures += 1;
            ep.consecutive_failures >= svc.config.consecutive_errors
        }; // entry lock released here

        if !should_eject {
            return;
        }

        // Steps 2+3: held together under eject_lock so that concurrent record_failure
        // calls for *different* backends cannot both pass the percent-check simultaneously
        // and exceed max_ejection_percent.
        let _eject_guard = svc.eject_lock.lock().unwrap_or_else(|e| e.into_inner());

        // Step 2: check max_ejection_percent.
        // Fall back to the observed endpoint count when the caller-provided total is 0
        // (e.g., selector snapshot momentarily empty during scale-down). Without this
        // fallback the entire percent cap would be skipped and concurrent failures could
        // eject every tracked backend regardless of `maxEjectionPercent`.
        if svc.config.max_ejection_percent > 0 {
            let effective_total = if total_endpoints > 0 {
                total_endpoints
            } else {
                svc.endpoints.len().max(1)
            };
            let currently_ejected = svc.endpoints.iter().filter(|e| e.value().ejected_at.is_some()).count();
            let max_ejectable = (effective_total as u64 * svc.config.max_ejection_percent as u64 / 100) as usize;
            if currently_ejected >= max_ejectable.max(1) {
                return;
            }
        }

        // Step 3: actually eject (re-acquire DashMap shard lock).
        // Trailing `;` is required: without it the Option<RefMut> temporary is a
        // trailing expression whose lifetime extends to the enclosing block end,
        // overlapping _eject_guard's borrow of the same Arc — the compiler rejects it.
        if let Some(mut ep) = svc.endpoints.get_mut(&backend_hash) {
            ep.ejected_at = Some(Instant::now());
            crate::observe::metrics::record_cb_ejection(service_key);
        };
    }

    pub fn record_success(&self, service_key: &str, backend_hash: u64) {
        let svc = match self.services.get(service_key) {
            Some(s) => Arc::clone(&s),
            None => return,
        };

        // Remove the entry on success: a healthy backend goes back to the default
        // "no entry = no tracked failures" state. If the removed entry happened to be
        // ejected (an in-flight request completing after ejection took effect), pair the
        // removal with `record_cb_recovery` so the gauge stays balanced.
        if let Some((_, ep)) = svc.endpoints.remove(&backend_hash) {
            if ep.ejected_at.is_some() {
                crate::observe::metrics::record_cb_recovery(service_key);
            }
        }
    }

    #[inline]
    pub fn has_service(&self, service_key: &str) -> bool {
        self.services.contains_key(service_key)
    }

    /// Decrement gauge for any currently-ejected endpoints in a service state.
    fn drain_ejected_gauge(service_key: &str, svc: &ServiceCbState) {
        let ejected = svc.endpoints.iter().filter(|e| e.value().ejected_at.is_some()).count();
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
        if let Some((_, ep)) = svc.endpoints.remove(&hash) {
            if ep.ejected_at.is_some() {
                crate::observe::metrics::record_cb_recovery(service_key);
            }
        }
    }

    /// Record failure using Pingora SocketAddr (convenience for hook callsites).
    pub fn record_failure_by_addr(&self, service_key: &str, addr: &pingora_core::protocols::l4::socket::SocketAddr) {
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
        self.record_failure(service_key, hash, total);
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

    fn make_config(errors: u32, seconds: u64, max_pct: u32) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            consecutive_errors: errors,
            ejection_seconds: seconds,
            max_ejection_percent: max_pct,
        }
    }

    #[test]
    fn test_no_config_means_not_ejected() {
        let store = CircuitBreakerStore::new();
        assert!(!store.is_ejected("default/svc1", 123));
    }

    #[test]
    fn test_record_failure_ejects_after_threshold() {
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(3, 30, 0)));

        for _ in 0..2 {
            store.record_failure("default/svc1", 111, 2);
            assert!(!store.is_ejected("default/svc1", 111));
        }

        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_success_resets_failures() {
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(3, 30, 0)));

        store.record_failure("default/svc1", 111, 2);
        store.record_failure("default/svc1", 111, 2);
        store.record_success("default/svc1", 111);
        store.record_failure("default/svc1", 111, 2);
        store.record_failure("default/svc1", 111, 2);

        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_auto_recovery_after_cooldown() {
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 0)));

        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));

        std::thread::sleep(std::time::Duration::from_secs(1));

        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_max_ejection_percent() {
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 30, 50)));

        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));

        store.record_failure("default/svc1", 222, 2);
        assert!(!store.is_ejected("default/svc1", 222));
    }

    #[test]
    fn test_max_ejection_percent_concurrent_no_over_eject() {
        use std::sync::{Arc, Barrier};

        // 4 backends, 50% limit → at most 2 may be ejected regardless of concurrency.
        let store = Arc::new(CircuitBreakerStore::new());
        store.set_service_config("default/svc1", Some(make_config(1, 30, 50)));

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
                    store.record_failure("default/svc1", hash, 4);
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
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 30, 0)));
        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));

        store.remove_service("default/svc1");
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_record_success_on_ejected_does_not_leak_gauge() {
        // Regression for g7-006 Issue A: record_success removing an ejected entry must
        // fire record_cb_recovery so the CB_EJECTED_ENDPOINTS gauge stays balanced.
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 30, 0)));

        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));

        // record_success on an ejected backend should remove the entry. After removal,
        // is_ejected returns false (no entry = healthy). The +1/-1 gauge symmetry is
        // verified by inspection of the removal branch in record_success itself.
        store.record_success("default/svc1", 111);
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_percent_cap_applies_concurrently_when_total_endpoints_is_zero() {
        // Regression for g7-006 Issue B under concurrency: 4 backends report failures
        // simultaneously while the selector snapshot is empty (total_endpoints == 0).
        // With the fallback to observed endpoint count, the percent cap must still
        // bound the number of ejected backends.
        use std::sync::{Arc, Barrier};

        let store = Arc::new(CircuitBreakerStore::new());
        // 50% cap. Once endpoints.len() reaches 4, max_ejectable = 2.
        store.set_service_config("default/svc1", Some(make_config(1, 30, 50)));

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
                    store.record_failure("default/svc1", hash, 0);
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
        // Regression for g7-006 Issue B: when the selector snapshot is momentarily empty
        // (total_endpoints == 0), the percent guard must still apply using the observed
        // endpoint count as a fallback, otherwise concurrent failures would over-eject.
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 30, 50)));

        // First failure with total=0 establishes the endpoint and ejects (max(1) of 50% of 1).
        store.record_failure("default/svc1", 111, 0);
        assert!(store.is_ejected("default/svc1", 111));

        // Second backend tries to eject with total=0 — observed endpoints are now 2, the
        // 50% cap is 1, and 1 backend is already ejected, so this must be rejected.
        store.record_failure("default/svc1", 222, 0);
        assert!(!store.is_ejected("default/svc1", 222));
    }

    #[test]
    fn test_config_change_resets_state_unchanged_preserves() {
        // Regression for backends-F004: the atomic entry() swap must preserve the original
        // semantics — a *changed* config rebuilds the service (clearing ejection state),
        // while an *unchanged* config is a no-op that keeps the existing ejection state.
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 30, 0)));
        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));

        // Re-applying the identical config must not wipe the ejection state.
        store.set_service_config("default/svc1", Some(make_config(1, 30, 0)));
        assert!(store.is_ejected("default/svc1", 111));

        // Applying a different config rebuilds the service with empty endpoints.
        store.set_service_config("default/svc1", Some(make_config(5, 30, 0)));
        assert!(!store.is_ejected("default/svc1", 111));
    }

    #[test]
    fn test_concurrent_config_update_no_panic_and_key_present() {
        // Regression for backends-F004: concurrent set_service_config calls for the same key
        // must never panic and must never leave the key absent (the old remove+insert form had
        // a window where the entry was missing). After the storm one config must be installed.
        use std::sync::{Arc, Barrier};

        let store = Arc::new(CircuitBreakerStore::new());
        store.set_service_config("default/svc1", Some(make_config(1, 30, 0)));

        let barrier = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4u32)
            .map(|i| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    // Each writer applies a distinct config to force the changed-path swap.
                    store.set_service_config("default/svc1", Some(make_config(i + 2, 30, 0)));
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
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 30, 0)));
        store.set_service_config("default/svc2", Some(make_config(1, 30, 0)));

        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));
        assert!(!store.is_ejected("default/svc2", 111));
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_zero() {
        let store = CircuitBreakerStore::new();
        assert_eq!(store.service_count(), 0);
        assert!(store.keys().is_empty());
    }
}
