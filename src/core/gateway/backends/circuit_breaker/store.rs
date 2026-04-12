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

    pub fn set_service_config(&self, service_key: &str, config: Option<CircuitBreakerConfig>) {
        match config {
            Some(cfg) => {
                if let Some(existing) = self.services.get(service_key) {
                    let existing_cfg = &existing.config;
                    if existing_cfg.consecutive_errors != cfg.consecutive_errors
                        || existing_cfg.ejection_seconds != cfg.ejection_seconds
                        || existing_cfg.max_ejection_percent != cfg.max_ejection_percent
                    {
                        drop(existing);
                        if let Some((_, old_svc)) = self.services.remove(service_key) {
                            Self::drain_ejected_gauge(service_key, &old_svc);
                        }
                        self.services.insert(
                            service_key.to_string(),
                            Arc::new(ServiceCbState {
                                config: cfg,
                                endpoints: DashMap::new(),
                            }),
                        );
                    }
                } else {
                    self.services.insert(
                        service_key.to_string(),
                        Arc::new(ServiceCbState {
                            config: cfg,
                            endpoints: DashMap::new(),
                        }),
                    );
                }
            }
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

        if ejected_at.elapsed() >= svc.config.ejection_duration() {
            drop(ep);
            svc.endpoints.remove(&backend_hash);
            crate::core::gateway::observe::metrics::record_cb_recovery(service_key);
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
        let (should_eject, failures) = {
            let mut ep = svc.endpoints.entry(backend_hash).or_default();
            if ep.ejected_at.is_some() {
                return;
            }
            ep.consecutive_failures += 1;
            (
                ep.consecutive_failures >= svc.config.consecutive_errors,
                ep.consecutive_failures,
            )
        }; // entry lock released here

        if !should_eject {
            return;
        }

        // Step 2: check max_ejection_percent (no entry lock held, safe to iterate)
        if svc.config.max_ejection_percent > 0 && total_endpoints > 0 {
            let currently_ejected = svc.endpoints.iter().filter(|e| e.value().ejected_at.is_some()).count();
            let max_ejectable = (total_endpoints as u64 * svc.config.max_ejection_percent as u64 / 100) as usize;
            if currently_ejected >= max_ejectable.max(1) {
                return;
            }
        }

        // Step 3: actually eject (re-acquire lock)
        if let Some(mut ep) = svc.endpoints.get_mut(&backend_hash) {
            ep.ejected_at = Some(Instant::now());
            tracing::warn!(
                component = "circuit_breaker",
                service = service_key,
                backend_hash = backend_hash,
                consecutive_failures = failures,
                ejection_seconds = svc.config.ejection_seconds,
                "Backend endpoint ejected"
            );
            crate::core::gateway::observe::metrics::record_cb_ejection(service_key);
        };
    }

    pub fn record_success(&self, service_key: &str, backend_hash: u64) {
        let svc = match self.services.get(service_key) {
            Some(s) => Arc::clone(&s),
            None => return,
        };

        // Success → remove entry entirely to avoid memory growth from stale endpoints.
        // Only healthy endpoints with prior failures have entries; removing returns them
        // to the default state (no entry = healthy).
        svc.endpoints.remove(&backend_hash);
    }

    #[inline]
    pub fn has_service(&self, service_key: &str) -> bool {
        self.services.contains_key(service_key)
    }

    /// Decrement gauge for any currently-ejected endpoints in a service state.
    fn drain_ejected_gauge(service_key: &str, svc: &ServiceCbState) {
        let ejected = svc.endpoints.iter().filter(|e| e.value().ejected_at.is_some()).count();
        for _ in 0..ejected {
            crate::core::gateway::observe::metrics::record_cb_recovery(service_key);
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
        let hash = crate::core::gateway::backends::health::check::backend_hash(&backend);
        if let Some((_, ep)) = svc.endpoints.remove(&hash) {
            if ep.ejected_at.is_some() {
                crate::core::gateway::observe::metrics::record_cb_recovery(service_key);
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
        let hash = crate::core::gateway::backends::health::check::backend_hash(&backend);
        let total = crate::core::gateway::backends::get_backends_for_service(service_key).len();
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
        let hash = crate::core::gateway::backends::health::check::backend_hash(&backend);
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
    fn test_remove_service_clears_state() {
        let store = CircuitBreakerStore::new();
        store.set_service_config("default/svc1", Some(make_config(1, 30, 0)));
        store.record_failure("default/svc1", 111, 2);
        assert!(store.is_ejected("default/svc1", 111));

        store.remove_service("default/svc1");
        assert!(!store.is_ejected("default/svc1", 111));
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
