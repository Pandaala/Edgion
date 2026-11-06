use crate::backends::discovery::ServiceDiscoveryCoordinator;
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

        // Per-service policy config is owned by EdgionBackendTrafficPolicy;
        // membership and runtime cells are owned by the coordinator. Cached DNS
        // answers carry no Service identity (hostname, resolver and family
        // describe the answer), so a removed Service leaves nothing to reclaim.
        self.coordinator.replace_services(data);
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
        self.coordinator.update_services(add, update, remove);
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/discovery/services/conf_handler_impl_test.rs"]
mod tests;
