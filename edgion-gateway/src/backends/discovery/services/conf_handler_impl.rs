use crate::backends::discovery::{get_global_dns_cache, ServiceDiscoveryCoordinator};
use edgion_common::conf_sync::traits::ConfHandler;
use k8s_openapi::api::core::v1::Service;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

struct ServiceHandler {
    coordinator: Arc<ServiceDiscoveryCoordinator>,
}

pub fn create_service_handler(
    coordinator: Arc<ServiceDiscoveryCoordinator>,
) -> Box<dyn ConfHandler<Service> + Send + Sync> {
    Box::new(ServiceHandler { coordinator })
}

impl ConfHandler<Service> for ServiceHandler {
    fn full_set(&self, data: &HashMap<String, Arc<Service>>) {
        tracing::info!(component = "service_store", cnt = data.len(), "full set");

        // Snapshot the previous key set before the swap so Self-resolved
        // ExternalName addresses can be dropped from the DNS cache. Per-service
        // policy config is owned by EdgionBackendTrafficPolicy; membership and
        // runtime cells are owned by the coordinator.
        let old_keys: HashSet<String> = self.coordinator.load_generation().services.keys().cloned().collect();
        self.coordinator.replace_services(data);

        let incoming_keys: HashSet<&String> = data.keys().collect();
        let removed_keys: HashSet<String> = old_keys
            .into_iter()
            .filter(|old_key| !incoming_keys.contains(old_key))
            .collect();
        if !removed_keys.is_empty() {
            get_global_dns_cache().invalidate_services(&removed_keys);
        }
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<Service>>,
        update: HashMap<String, Arc<Service>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "service_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );
        self.coordinator.update_services(add, update, remove.clone());
        if !remove.is_empty() {
            get_global_dns_cache().invalidate_services(&remove);
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/discovery/services/conf_handler_impl_test.rs"]
mod tests;
