use arc_swap::ArcSwap;
use k8s_openapi::api::core::v1::Service;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

static GLOBAL_SERVICE_STORE: LazyLock<Arc<ServiceStore>> = LazyLock::new(|| Arc::new(ServiceStore::new()));

pub fn get_global_service_store() -> Arc<ServiceStore> {
    GLOBAL_SERVICE_STORE.clone()
}

/// Type alias for the service map
type ServiceMap = HashMap<String, Service>;

pub struct ServiceStore {
    services: ArcSwap<ServiceMap>,
}

impl Default for ServiceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceStore {
    pub fn new() -> Self {
        Self {
            services: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Check if a service exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.services.load();
        map.contains_key(key)
    }

    /// Get a service by key
    pub fn get(&self, key: &str) -> Option<Service> {
        let map = self.services.load();
        map.get(key).cloned()
    }

    /// Execute a function with the service reference
    pub fn with_service<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&Service) -> R,
    {
        let map = self.services.load();
        map.get(key).map(f)
    }

    /// Replace all services atomically
    pub fn replace_all(&self, services: HashMap<String, Service>) {
        self.services.store(Arc::new(services));
    }

    /// Update services atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Service>, remove: &HashSet<String>) {
        let current = self.services.load();
        let current_map: &ServiceMap = &current;
        let mut new_map: ServiceMap = current_map.clone();

        for key in remove {
            new_map.remove(key);
        }
        for (key, service) in add_or_update {
            new_map.insert(key, service);
        }

        self.services.store(Arc::new(new_map));
    }

    /// Number of services currently in the store.
    pub fn count(&self) -> usize {
        self.services.load().len()
    }

    /// Keys currently in the store (snapshot copy). Used by leak detection.
    pub fn keys(&self) -> Vec<String> {
        self.services.load().keys().cloned().collect()
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;
    use k8s_openapi::api::core::v1::Service;

    fn svc(name: &str) -> Service {
        Service {
            metadata: kube::api::ObjectMeta {
                name: Some(name.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn count_returns_zero_when_empty() {
        let store = ServiceStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }

    #[test]
    fn count_and_keys_track_inserts_and_removes() {
        let store = ServiceStore::new();
        let mut add = HashMap::new();
        add.insert("default/a".to_string(), svc("a"));
        add.insert("default/b".to_string(), svc("b"));
        store.update(add, &HashSet::new());
        assert_eq!(store.count(), 2);
        let mut keys = store.keys();
        keys.sort();
        assert_eq!(keys, vec!["default/a".to_string(), "default/b".to_string()]);

        let mut remove = HashSet::new();
        remove.insert("default/a".to_string());
        store.update(HashMap::new(), &remove);
        assert_eq!(store.count(), 1);
        assert_eq!(store.keys(), vec!["default/b".to_string()]);
    }
}
