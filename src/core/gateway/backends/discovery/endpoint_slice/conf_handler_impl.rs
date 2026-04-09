use super::get_roundrobin_store;
use crate::core::common::conf_sync::traits::{arc_map_into_owned, arc_map_to_owned, ConfHandler};
use crate::core::gateway::backends::discovery::conf_handler_common::{update_affected_lbs, update_all_existing_lbs};
use crate::core::gateway::backends::health::check::{
    annotation::parse_health_check_annotation, get_hc_config_store, get_health_check_manager,
};
use crate::core::gateway::lb::runtime_state;
use crate::types::resources::health_check::ActiveHealthCheckConfig;
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

    fn resolve_endpoint_slice_config_for_service(&self, service_key: &str) -> Option<ActiveHealthCheckConfig> {
        let roundrobin_store = get_roundrobin_store();
        let slices = roundrobin_store.get_slices_for_service(service_key)?;

        let mut selected: Option<(ActiveHealthCheckConfig, String)> = None;
        for slice in slices {
            let Some(cfg) = parse_health_check_annotation(&slice.metadata) else {
                continue;
            };
            let slice_name = slice.metadata.name.clone().unwrap_or_default();

            if let Some((existing_cfg, selected_slice)) = &selected {
                if existing_cfg != &cfg {
                    tracing::warn!(
                        service = %service_key,
                        selected_slice = %selected_slice,
                        conflict_slice = %slice_name,
                        "Conflicting EndpointSlice health-check annotations detected; disable endpoint-level config and fallback to Service"
                    );
                    return None;
                }
            } else {
                selected = Some((cfg, slice_name));
            }
        }

        selected.map(|(cfg, _)| cfg)
    }

    fn sync_health_check_configs_for_services(&self, service_keys: &HashSet<String>) {
        let config_store = get_hc_config_store();
        let hc_manager = get_health_check_manager();

        for service_key in service_keys {
            let active_config = self.resolve_endpoint_slice_config_for_service(service_key);
            config_store.set_endpoint_slice_config(service_key, active_config);
            hc_manager.reconcile_service(service_key);
        }
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

        let config_store = get_hc_config_store();
        let old_keys: HashSet<String> = config_store.endpoint_slice_keys().into_iter().collect();

        // 1. Only update RoundRobin store's data layer (shared data layer)
        let roundrobin_store = get_roundrobin_store();
        let data_owned = arc_map_to_owned(data);
        let all_services = roundrobin_store.replace_data_only(data_owned);

        // 2. Update all existing LBs in ALL stores
        // This handles relist scenario where data might have changed
        update_all_existing_lbs(&*roundrobin_store);

        // 3. Sync EndpointSlice-level health check config
        self.sync_health_check_configs_for_services(&all_services);

        // 4. Invalidate selector caches for all services (data may have changed)
        for service_key in &all_services {
            runtime_state::invalidate_selector_cache(service_key);
        }

        // 5. Clear stale services (removed by full_set)
        let stale_services: HashSet<String> = old_keys.difference(&all_services).cloned().collect();
        if !stale_services.is_empty() {
            let cb_store = crate::core::gateway::backends::get_circuit_breaker_store();
            for service_key in &stale_services {
                config_store.set_endpoint_slice_config(service_key, None);
                get_health_check_manager().reconcile_service(service_key);
                runtime_state::remove_service(service_key);
                cb_store.remove_service(service_key);
            }
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

        // 3. Sync EndpointSlice-level health check config
        self.sync_health_check_configs_for_services(&affected_services);

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
