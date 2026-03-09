use super::get_endpoint_roundrobin_store;
use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::backends::health::check::{
    annotation::parse_health_check_annotation, get_hc_config_store, get_health_check_manager,
};
use crate::core::gateway::lb::runtime_state;
use k8s_openapi::api::core::v1::Endpoints;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::collections::{HashMap, HashSet};

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

/// Handler for Endpoints configuration updates
///
/// Design: Only the RoundRobin store maintains data + LB.
/// LeastConn/EWMA/ConsistentHash read backends from RR at selection time,
/// so this handler only updates the RR data layer and refreshes existing RR LBs.
pub struct EndpointHandler;

impl EndpointHandler {
    pub fn new() -> Self {
        Self
    }

    /// Update all existing RoundRobin LBs (used after full_set/relist).
    fn update_all_existing_lbs(&self) {
        let roundrobin_store = get_endpoint_roundrobin_store();
        for service_key in roundrobin_store.get_existing_service_keys() {
            roundrobin_store.update_lb_if_exists(&service_key);
        }
    }

    /// Update affected RoundRobin LBs and clean stale runtime state.
    fn update_affected_lbs(&self, affected_services: &HashSet<String>) {
        let roundrobin_store = get_endpoint_roundrobin_store();

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

    fn resolve_endpoint_config_for_service(
        &self,
        service_key: &str,
    ) -> Option<crate::types::resources::health_check::ActiveHealthCheckConfig> {
        let roundrobin_store = get_endpoint_roundrobin_store();
        let endpoint = roundrobin_store.get_endpoint_for_service(service_key)?;
        parse_health_check_annotation(&endpoint.metadata)
    }

    fn sync_health_check_configs_for_services(&self, service_keys: &HashSet<String>) {
        let config_store = get_hc_config_store();
        let hc_manager = get_health_check_manager();

        for service_key in service_keys {
            let active_config = self.resolve_endpoint_config_for_service(service_key);
            config_store.set_endpoint_config(service_key, active_config);
            hc_manager.reconcile_service(service_key);
        }
    }
}

impl Default for EndpointHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Create an EndpointStore handler for registration with ConfigClient
pub fn create_endpoint_handler() -> Box<dyn ConfHandler<Endpoints> + Send + Sync> {
    Box::new(EndpointHandler::new())
}

impl ConfHandler<Endpoints> for EndpointHandler {
    fn full_set(&self, data: &HashMap<String, Endpoints>) {
        tracing::info!(
            component = "endpoint_handler",
            cnt = data.len(),
            "full set - updating data layer"
        );

        let config_store = get_hc_config_store();
        let old_keys: HashSet<String> = config_store.endpoint_keys().into_iter().collect();

        // 1. Only update RoundRobin store's data layer (shared data layer)
        let roundrobin_store = get_endpoint_roundrobin_store();
        let all_services = roundrobin_store.replace_data_only(data.clone());

        // 2. Update all existing LBs in ALL stores
        // This handles relist scenario where data might have changed
        self.update_all_existing_lbs();

        // 3. Sync Endpoints-level health check config
        self.sync_health_check_configs_for_services(&all_services);

        // 4. Invalidate selector caches for all services (data may have changed)
        for service_key in &all_services {
            runtime_state::invalidate_selector_cache(service_key);
        }

        // 5. Clear stale services (removed by full_set)
        let stale_services: HashSet<String> = old_keys.difference(&all_services).cloned().collect();
        if !stale_services.is_empty() {
            for service_key in &stale_services {
                config_store.set_endpoint_config(service_key, None);
                get_health_check_manager().reconcile_service(service_key);
                runtime_state::remove_service(service_key);
            }
        }

        tracing::info!(
            component = "endpoint_handler",
            total_eps = data.len(),
            total_services = all_services.len(),
            "Full set completed (lazy LB creation enabled)"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Endpoints>,
        update: HashMap<String, Endpoints>,
        remove: HashSet<String>,
    ) {
        let add_count = add.len();
        let update_count = update.len();
        let remove_count = remove.len();

        // 1. Only update RoundRobin store's data layer (shared data layer)
        let roundrobin_store = get_endpoint_roundrobin_store();
        let affected_services = roundrobin_store.update_data_only(add, update, &remove);

        // 2. Update affected LBs in ALL stores
        self.update_affected_lbs(&affected_services);

        // 3. Sync Endpoints-level health check config
        self.sync_health_check_configs_for_services(&affected_services);

        tracing::info!(
            component = "endpoint_handler",
            add_count,
            update_count,
            remove_count,
            affected_services = affected_services.len(),
            "Partial update completed (lazy LB creation enabled)"
        );
    }
}
