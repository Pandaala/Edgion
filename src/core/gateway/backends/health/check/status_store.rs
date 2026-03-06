use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

static HEALTH_STATUS_STORE: LazyLock<Arc<HealthStatusStore>> = LazyLock::new(|| Arc::new(HealthStatusStore::new()));

pub fn get_health_status_store() -> Arc<HealthStatusStore> {
    HEALTH_STATUS_STORE.clone()
}

#[derive(Debug, Clone)]
struct BackendHealthState {
    healthy: bool,
    consecutive_successes: u32,
    consecutive_failures: u32,
    last_check_time: Instant,
    last_transition_time: Option<Instant>,
}

impl Default for BackendHealthState {
    fn default() -> Self {
        Self {
            healthy: true,
            consecutive_successes: 0,
            consecutive_failures: 0,
            last_check_time: Instant::now(),
            last_transition_time: None,
        }
    }
}

struct ServiceHealthState {
    /// Current backends tracked for this service.
    tracked_backends: ArcSwap<HashSet<u64>>,
    /// Snapshot used by request path: unhealthy backend hashes.
    unhealthy_backends: ArcSwap<HashSet<u64>>,
    /// Probe counters/state for each backend.
    states: Mutex<HashMap<u64, BackendHealthState>>,
}

impl Default for ServiceHealthState {
    fn default() -> Self {
        Self {
            tracked_backends: ArcSwap::from_pointee(HashSet::new()),
            unhealthy_backends: ArcSwap::from_pointee(HashSet::new()),
            states: Mutex::new(HashMap::new()),
        }
    }
}

/// Thread-safe store for external backend health status.
///
/// Health state is isolated by service_key to avoid cross-service backend state leakage.
pub struct HealthStatusStore {
    /// service_key -> service health state
    services: ArcSwap<HashMap<String, Arc<ServiceHealthState>>>,
    /// Serialize map-level add/remove operations.
    services_update_lock: Mutex<()>,
}

impl Default for HealthStatusStore {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthStatusStore {
    pub fn new() -> Self {
        Self {
            services: ArcSwap::from_pointee(HashMap::new()),
            services_update_lock: Mutex::new(()),
        }
    }

    /// Unknown service/backend defaults to healthy.
    #[inline]
    pub fn is_healthy(&self, service_key: &str, backend_hash: u64) -> bool {
        let Some(service_state) = self.services.load().get(service_key).cloned() else {
            return true;
        };
        !service_state.unhealthy_backends.load().contains(&backend_hash)
    }

    #[inline]
    pub fn has_service(&self, service_key: &str) -> bool {
        self.services.load().contains_key(service_key)
    }

    pub fn record_check(
        &self,
        service_key: &str,
        backend_hash: u64,
        success: bool,
        healthy_threshold: u32,
        unhealthy_threshold: u32,
    ) {
        let Some(service_state) = self.services.load().get(service_key).cloned() else {
            return;
        };
        if !service_state.tracked_backends.load().contains(&backend_hash) {
            return;
        }

        let healthy_threshold = healthy_threshold.max(1);
        let unhealthy_threshold = unhealthy_threshold.max(1);

        let now = Instant::now();
        let mut states = service_state.states.lock().expect("lock service states");
        let entry = states.entry(backend_hash).or_default();
        let was_healthy = entry.healthy;
        entry.last_check_time = now;

        if success {
            entry.consecutive_failures = 0;
            entry.consecutive_successes = entry.consecutive_successes.saturating_add(1);
            if !entry.healthy && entry.consecutive_successes >= healthy_threshold {
                entry.healthy = true;
                entry.last_transition_time = Some(now);
            }
        } else {
            entry.consecutive_successes = 0;
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
            if entry.healthy && entry.consecutive_failures >= unhealthy_threshold {
                entry.healthy = false;
                entry.last_transition_time = Some(now);
            }
        }

        let is_healthy = entry.healthy;
        drop(states);

        if was_healthy != is_healthy {
            let mut unhealthy_snapshot = (**service_state.unhealthy_backends.load()).clone();
            if is_healthy {
                unhealthy_snapshot.remove(&backend_hash);
            } else {
                unhealthy_snapshot.insert(backend_hash);
            }
            service_state.unhealthy_backends.store(Arc::new(unhealthy_snapshot));
        }
    }

    pub fn register_service(&self, service_key: &str, backend_hashes: Vec<u64>) {
        let service_state = self.get_or_create_service_state(service_key);
        let backend_set: HashSet<u64> = backend_hashes.into_iter().collect();
        if service_state.tracked_backends.load().as_ref() == &backend_set {
            return;
        }

        service_state.tracked_backends.store(Arc::new(backend_set.clone()));

        let mut states = service_state.states.lock().expect("lock service states");
        states.retain(|hash, _| backend_set.contains(hash));
        for hash in &backend_set {
            states.entry(*hash).or_default();
        }

        let unhealthy_snapshot: HashSet<u64> = states
            .iter()
            .filter_map(|(hash, state)| if state.healthy { None } else { Some(*hash) })
            .collect();
        drop(states);

        service_state.unhealthy_backends.store(Arc::new(unhealthy_snapshot));
    }

    pub fn unregister_service(&self, service_key: &str) {
        let _guard = self.services_update_lock.lock().expect("lock services update");
        let current = self.services.load();
        if !current.contains_key(service_key) {
            return;
        }

        let mut new_map = (**current).clone();
        new_map.remove(service_key);
        self.services.store(Arc::new(new_map));
    }

    fn get_or_create_service_state(&self, service_key: &str) -> Arc<ServiceHealthState> {
        if let Some(existing) = self.services.load().get(service_key).cloned() {
            return existing;
        }

        let _guard = self.services_update_lock.lock().expect("lock services update");
        let current = self.services.load();
        if let Some(existing) = current.get(service_key).cloned() {
            return existing;
        }

        let mut new_map = (**current).clone();
        let service_state = Arc::new(ServiceHealthState::default());
        new_map.insert(service_key.to_string(), service_state.clone());
        self.services.store(Arc::new(new_map));
        service_state
    }
}

#[cfg(test)]
impl HealthStatusStore {
    fn service_count(&self) -> usize {
        self.services.load().len()
    }

    fn unhealthy_count(&self, service_key: &str) -> usize {
        self.services
            .load()
            .get(service_key)
            .map(|svc| svc.unhealthy_backends.load().len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_unknown_backend_defaults_healthy() {
        let store = HealthStatusStore::new();
        assert!(store.is_healthy("default/svc", 12345));
    }

    #[test]
    fn test_consecutive_failures_trigger_unhealthy() {
        let store = HealthStatusStore::new();
        let svc = "default/svc";
        let hash = 1_u64;
        store.register_service(svc, vec![hash]);

        store.record_check(svc, hash, false, 2, 3);
        assert!(store.is_healthy(svc, hash));
        store.record_check(svc, hash, false, 2, 3);
        assert!(store.is_healthy(svc, hash));
        store.record_check(svc, hash, false, 2, 3);
        assert!(!store.is_healthy(svc, hash));
        assert_eq!(store.unhealthy_count(svc), 1);
    }

    #[test]
    fn test_consecutive_successes_trigger_healthy() {
        let store = HealthStatusStore::new();
        let svc = "default/svc";
        let hash = 2_u64;
        store.register_service(svc, vec![hash]);

        store.record_check(svc, hash, false, 2, 1);
        assert!(!store.is_healthy(svc, hash));
        store.record_check(svc, hash, true, 2, 1);
        assert!(!store.is_healthy(svc, hash));
        store.record_check(svc, hash, true, 2, 1);
        assert!(store.is_healthy(svc, hash));
        assert_eq!(store.unhealthy_count(svc), 0);
    }

    #[test]
    fn test_success_resets_failure_counter() {
        let store = HealthStatusStore::new();
        let svc = "default/svc";
        let hash = 3_u64;
        store.register_service(svc, vec![hash]);

        store.record_check(svc, hash, false, 2, 3);
        store.record_check(svc, hash, false, 2, 3);
        store.record_check(svc, hash, true, 2, 3);
        store.record_check(svc, hash, false, 2, 3);
        assert!(store.is_healthy(svc, hash));
    }

    #[test]
    fn test_failure_resets_success_counter() {
        let store = HealthStatusStore::new();
        let svc = "default/svc";
        let hash = 4_u64;
        store.register_service(svc, vec![hash]);

        store.record_check(svc, hash, false, 2, 1);
        assert!(!store.is_healthy(svc, hash));
        store.record_check(svc, hash, true, 2, 1);
        store.record_check(svc, hash, false, 2, 1);
        assert!(!store.is_healthy(svc, hash));
    }

    #[test]
    fn test_register_unregister_service_cleanup() {
        let store = HealthStatusStore::new();
        let svc = "default/svc";
        store.register_service(svc, vec![10, 20]);
        assert!(store.has_service(svc));
        assert!(store.is_healthy(svc, 10));

        store.unregister_service(svc);
        assert!(!store.has_service(svc));
        assert!(store.is_healthy(svc, 10));
        assert_eq!(store.service_count(), 0);
    }

    #[test]
    fn test_cross_service_backend_isolation() {
        let store = HealthStatusStore::new();
        let shared = 88_u64;
        let svc_a = "default/svc-a";
        let svc_b = "default/svc-b";
        store.register_service(svc_a, vec![shared]);
        store.register_service(svc_b, vec![shared]);

        store.record_check(svc_a, shared, false, 1, 1);
        assert!(!store.is_healthy(svc_a, shared));
        assert!(store.is_healthy(svc_b, shared));
    }

    #[test]
    fn test_concurrent_access() {
        let store = Arc::new(HealthStatusStore::new());
        let svc = "default/svc";
        let hash = 99_u64;
        store.register_service(svc, vec![hash]);

        let mut threads = Vec::new();
        for _ in 0..8 {
            let store_ref = store.clone();
            threads.push(thread::spawn(move || {
                for i in 0..1000 {
                    store_ref.record_check(svc, hash, i % 2 == 0, 2, 3);
                    let _ = store_ref.is_healthy(svc, hash);
                }
            }));
        }

        for handle in threads {
            handle.join().expect("join thread");
        }
    }
}
