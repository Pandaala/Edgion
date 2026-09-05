use crate::backends::discovery::ServiceDiscoveryCoordinator;
use edgion_common::conf_sync::traits::ConfHandler;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

struct EndpointSliceHandler {
    coordinator: Arc<ServiceDiscoveryCoordinator>,
}

pub fn create_ep_slice_handler(
    coordinator: Arc<ServiceDiscoveryCoordinator>,
) -> Box<dyn ConfHandler<EndpointSlice> + Send + Sync> {
    Box::new(EndpointSliceHandler { coordinator })
}

impl ConfHandler<EndpointSlice> for EndpointSliceHandler {
    fn full_set(&self, data: &HashMap<String, Arc<EndpointSlice>>) {
        tracing::info!(component = "ep_slice_handler", cnt = data.len(), "full set");
        self.coordinator.replace_endpoint_slices(data);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EndpointSlice>>,
        update: HashMap<String, Arc<EndpointSlice>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "ep_slice_handler",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );
        self.coordinator.update_endpoint_slices(add, update, remove);
    }
}
