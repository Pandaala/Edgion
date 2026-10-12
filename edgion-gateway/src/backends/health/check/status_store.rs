use arc_swap::ArcSwap;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

static HEALTH_STATUS_STORE: LazyLock<Arc<HealthStatusStore>> = LazyLock::new(|| Arc::new(HealthStatusStore::new()));

pub fn get_health_status_store() -> Arc<HealthStatusStore> {
    HEALTH_STATUS_STORE.clone()
}

/// Concurrent registry of active-health Service markers for admin statistics.
///
/// Serialized manager reconciliation owns membership. Endpoint eligibility and
/// threshold counters belong exclusively to ServiceDiscoveryCoordinator.
pub struct HealthStatusStore {
    services: ArcSwap<HashSet<String>>,
    /// Serialize marker membership read-modify-publish operations.
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
            services: ArcSwap::from_pointee(HashSet::new()),
            services_update_lock: Mutex::new(()),
        }
    }

    /// Number of Services with an active-health marker.
    pub fn tracked_service_count(&self) -> usize {
        self.services.load().len()
    }

    /// Service keys currently tracked (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.services.load().iter().cloned().collect()
    }

    #[inline]
    pub fn has_service(&self, service_key: &str) -> bool {
        self.services.load().contains(service_key)
    }

    pub fn register_service(&self, service_key: &str) {
        let _guard = self.services_update_lock.lock();
        let current = self.services.load();
        if current.contains(service_key) {
            return;
        }
        let mut next = (**current).clone();
        next.insert(service_key.to_string());
        self.services.store(Arc::new(next));
    }

    pub fn unregister_service(&self, service_key: &str) {
        let _guard = self.services_update_lock.lock();
        let current = self.services.load();
        if !current.contains(service_key) {
            return;
        }

        let mut new_map = (**current).clone();
        new_map.remove(service_key);
        self.services.store(Arc::new(new_map));
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/health/check/status_store_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/backends/health/check/status_store_test/stats_tests.rs"]
mod stats_tests;
