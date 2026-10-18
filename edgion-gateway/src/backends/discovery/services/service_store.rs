use k8s_openapi::api::core::v1::Service;
use std::sync::{Arc, LazyLock};

use crate::backends::discovery::{
    get_service_discovery_coordinator, ServiceDiscoveryCoordinator, ServicePublishedState,
};

static GLOBAL_SERVICE_STORE: LazyLock<Arc<ServiceStore>> =
    LazyLock::new(|| Arc::new(ServiceStore::new(Arc::clone(get_service_discovery_coordinator()))));

pub fn get_global_service_store() -> Arc<ServiceStore> {
    GLOBAL_SERVICE_STORE.clone()
}

/// Read-only compatibility facade over the joint Service selector generation.
///
/// Production mutation is owned exclusively by `ServiceDiscoveryCoordinator`.
/// The facade performs one generation load and never exposes unavailable raw
/// Service input.
pub struct ServiceStore {
    coordinator: Arc<ServiceDiscoveryCoordinator>,
}

impl ServiceStore {
    pub fn new(coordinator: Arc<ServiceDiscoveryCoordinator>) -> Self {
        Self { coordinator }
    }

    pub fn contains(&self, key: &str) -> bool {
        matches!(
            self.coordinator.load_generation().service(key),
            Some(ServicePublishedState::Available(_))
        )
    }

    pub fn get(&self, key: &str) -> Option<Arc<Service>> {
        self.coordinator
            .load_generation()
            .available(key)
            .map(|service| Arc::clone(&service.raw.service))
    }

    pub fn with_service<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&Service) -> R,
    {
        let generation = self.coordinator.load_generation();
        generation.available(key).map(|service| f(&service.raw.service))
    }

    pub fn count(&self) -> usize {
        self.coordinator.load_generation().stats.service_sources
    }

    pub fn keys(&self) -> Vec<String> {
        let generation = self.coordinator.load_generation();
        let mut keys: Vec<String> = generation
            .services
            .iter()
            .filter_map(|(key, state)| matches!(state, ServicePublishedState::Available(_)).then_some(key.clone()))
            .collect();
        keys.sort();
        keys
    }

    #[cfg(test)]
    pub fn update(&self, add: std::collections::HashMap<String, Service>, remove: &std::collections::HashSet<String>) {
        self.coordinator.update_services(
            add.into_iter().map(|(key, value)| (key, Arc::new(value))).collect(),
            std::collections::HashMap::new(),
            remove.clone(),
        );
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/discovery/services/service_store_test.rs"]
mod tests;
