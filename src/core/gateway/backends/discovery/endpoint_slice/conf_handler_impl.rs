use super::get_roundrobin_store;
use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::backends::health::check::{
    annotation::parse_health_check_annotation, get_hc_config_store, get_health_check_manager,
};
use crate::core::gateway::lb::runtime_state;
use crate::types::resources::health_check::ActiveHealthCheckConfig;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::collections::{HashMap, HashSet};

/// Diff old vs new backend sets, remove stale entries, and invalidate selector caches.
fn cleanup_removed_backends(service_key: &str, old: &HashSet<SocketAddr>, new: &HashSet<SocketAddr>) {
    let removed: Vec<SocketAddr> = old.difference(new).cloned().collect();
    let added: Vec<SocketAddr> = new.difference(old).cloned().collect();

    if !removed.is_empty() || !added.is_empty() {
        runtime_state::invalidate_selector_cache(service_key);
    }

    if !removed.is_empty() {
        tracing::debug!(
            service_key = %service_key,
            removed_count = removed.len(),
            "Cleaning stale backend runtime state"
        );
        for addr in &removed {
            if runtime_state::get_count(service_key, addr) > 0 {
                runtime_state::mark_backend_draining(service_key, addr);
            } else {
                runtime_state::remove_backend(service_key, addr);
            }
        }
    }
}

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

    /// Update all existing RoundRobin LBs (used after full_set/relist).
    /// LeastConn/EWMA/ConsistentHash read from the RR backend list at selection
    /// time, so they don't need separate LB instances to update.
    fn update_all_existing_lbs(&self) {
        let roundrobin_store = get_roundrobin_store();
        for service_key in roundrobin_store.get_existing_service_keys() {
            roundrobin_store.update_lb_if_exists(&service_key);
        }
    }

    /// Update affected RoundRobin LBs and clean stale runtime state.
    fn update_affected_lbs(&self, affected_services: &HashSet<String>) {
        let roundrobin_store = get_roundrobin_store();

        for service_key in affected_services {
            let old_addrs: HashSet<SocketAddr> = roundrobin_store
                .get_backends_for_service(service_key)
                .into_iter()
                .map(|b| b.addr)
                .collect();

            roundrobin_store.update_lb_if_exists(service_key);

            let new_addrs: HashSet<SocketAddr> = roundrobin_store
                .get_backends_for_service(service_key)
                .into_iter()
                .map(|b| b.addr)
                .collect();
            cleanup_removed_backends(service_key, &old_addrs, &new_addrs);
        }
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
    fn full_set(&self, data: &HashMap<String, EndpointSlice>) {
        tracing::info!(
            component = "ep_slice_handler",
            cnt = data.len(),
            "full set - updating data layer"
        );

        let config_store = get_hc_config_store();
        let old_keys: HashSet<String> = config_store.endpoint_slice_keys().into_iter().collect();

        // 1. Only update RoundRobin store's data layer (shared data layer)
        let roundrobin_store = get_roundrobin_store();
        let all_services = roundrobin_store.replace_data_only(data.clone());

        // 2. Update all existing LBs in ALL stores
        // This handles relist scenario where data might have changed
        self.update_all_existing_lbs();

        // 3. Sync EndpointSlice-level health check config
        self.sync_health_check_configs_for_services(&all_services);

        // 4. Invalidate selector caches for all services (data may have changed)
        for service_key in &all_services {
            runtime_state::invalidate_selector_cache(service_key);
        }

        // 5. Clear stale services (removed by full_set)
        let stale_services: HashSet<String> = old_keys.difference(&all_services).cloned().collect();
        if !stale_services.is_empty() {
            for service_key in &stale_services {
                config_store.set_endpoint_slice_config(service_key, None);
                get_health_check_manager().reconcile_service(service_key);
                runtime_state::remove_service(service_key);
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
        add: HashMap<String, EndpointSlice>,
        update: HashMap<String, EndpointSlice>,
        remove: HashSet<String>,
    ) {
        let add_count = add.len();
        let update_count = update.len();
        let remove_count = remove.len();

        // 1. Only update RoundRobin store's data layer (shared data layer)
        let roundrobin_store = get_roundrobin_store();
        let affected_services = roundrobin_store.update_data_only(add, update, &remove);

        // 2. Update affected LBs in ALL stores
        self.update_affected_lbs(&affected_services);

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
