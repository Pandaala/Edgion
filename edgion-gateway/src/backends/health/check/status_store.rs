use arc_swap::ArcSwap;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};
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

    /// Number of services with tracked health state.
    pub fn tracked_service_count(&self) -> usize {
        self.services.load().len()
    }

    /// Service keys currently tracked (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.services.load().keys().cloned().collect()
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
        let mut states = service_state.states.lock();
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

        let mut states = service_state.states.lock();
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
        let _guard = self.services_update_lock.lock();
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

        let _guard = self.services_update_lock.lock();
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
include!("../../../../tests/support/backends/health/check/status_store_test/210_impl_item.rs");

#[cfg(test)]
#[path = "../../../../tests/unit/backends/health/check/status_store_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/backends/health/check/status_store_test/stats_tests.rs"]
mod stats_tests;
