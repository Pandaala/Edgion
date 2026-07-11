use super::get_roundrobin_store;
use crate::backends::discovery::conf_handler_common::{update_affected_lbs, update_all_existing_lbs};
use crate::lb::compiled_selector;
use crate::lb::runtime_state;
use edgion_common::conf_sync::traits::{arc_map_into_owned, arc_map_to_owned, ConfHandler};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Handler for EndpointSlice configuration updates
///
/// Design: Only the RoundRobin store maintains data + LB.
/// LeastConn/EWMA/ConsistentHash read backends from RR at selection time,
/// so this handler only updates the RR data layer and refreshes existing RR LBs.
pub struct EpSliceHandler;

impl EpSliceHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EpSliceHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Create an EpSliceStore handler for registration with ConfigClient
pub fn create_ep_slice_handler() -> Box<dyn ConfHandler<EndpointSlice> + Send + Sync> {
    Box::new(EpSliceHandler::new())
}

impl ConfHandler<EndpointSlice> for EpSliceHandler {
    fn full_set(&self, data: &HashMap<String, Arc<EndpointSlice>>) {
        tracing::info!(
            component = "ep_slice_handler",
            cnt = data.len(),
            "full set - updating data layer"
        );

        // Use cell keys so services are tracked and their cells dropped when the
        // service is deleted.
        let old_keys: HashSet<String> = compiled_selector::all_cell_keys().into_iter().collect();

        // 1. Only update RoundRobin store's data layer (shared data layer)
        let roundrobin_store = get_roundrobin_store();
        let data_owned = arc_map_to_owned(data);
        let all_services = roundrobin_store.replace_data_only(data_owned);

        // 2. Push fresh backends into cells for all known services
        update_all_existing_lbs(&*roundrobin_store, &all_services);

        // 3. Invalidate SP-affinity caches for all services (data may have changed)
        for service_key in &all_services {
            runtime_state::invalidate_selector_cache(service_key);
        }

        // 4. Clear stale services (removed by full_set). Per-service policy config
        // is owned by the EdgionBackendTrafficPolicy conf-handler, so only the
        // per-backend runtime state and selector cell are dropped here.
        let stale_services: HashSet<String> = old_keys.difference(&all_services).cloned().collect();
        for service_key in &stale_services {
            runtime_state::remove_service(service_key);
            compiled_selector::drop_cell(service_key);
        }

        tracing::info!(
            component = "ep_slice_handler",
            total_eps = data.len(),
            total_services = all_services.len(),
            "Full set completed (lazy LB creation enabled)"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EndpointSlice>>,
        update: HashMap<String, Arc<EndpointSlice>>,
        remove: HashSet<String>,
    ) {
        let add_count = add.len();
        let update_count = update.len();
        let remove_count = remove.len();

        // 1. Only update RoundRobin store's data layer (shared data layer)
        let roundrobin_store = get_roundrobin_store();
        let add_owned = arc_map_into_owned(add);
        let update_owned = arc_map_into_owned(update);
        let affected_services = roundrobin_store.update_data_only(add_owned, update_owned, &remove);

        // 2. Update affected LBs in ALL stores
        update_affected_lbs(&*roundrobin_store, &affected_services);

        // 3. Clean up stale services whose last EndpointSlice was removed.
        // Per-service policy config is owned by the EdgionBackendTrafficPolicy
        // conf-handler, so only per-backend runtime state and selector cells are
        // dropped here.
        if remove_count > 0 {
            for service_key in &affected_services {
                if roundrobin_store.get_slices_for_service(service_key).is_none() {
                    runtime_state::remove_service(service_key);
                    compiled_selector::drop_cell(service_key);
                }
            }
        }

        tracing::info!(
            component = "ep_slice_handler",
            add_count,
            update_count,
            remove_count,
            affected_services = affected_services.len(),
            "Partial update completed (lazy LB creation enabled)"
        );
    }
}
