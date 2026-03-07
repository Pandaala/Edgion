use super::{
    get_endpoint_consistent_store, get_endpoint_ewma_store, get_endpoint_leastconn_store, get_endpoint_roundrobin_store,
};
use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::backends::health::check::{
    annotation::parse_health_check_annotation, get_hc_config_store, get_health_check_manager,
};
use k8s_openapi::api::core::v1::Endpoints;
use std::collections::{HashMap, HashSet};

/// Handler for Endpoints configuration updates
///
/// Design: Uses shared data layer architecture
/// - Only RoundRobin store maintains the data layer
/// - Other algorithm stores (Consistent/LeastConn/Ewma) only maintain LB layer
/// - LBs are created on-demand via DCL pattern in data plane
/// - This handler only updates data layer and refreshes existing LBs
pub struct EndpointHandler;

impl EndpointHandler {
    pub fn new() -> Self {
        Self
    }

    /// Update all existing LBs in all stores (used after full_set/relist)
    fn update_all_existing_lbs(&self) {
        let roundrobin_store = get_endpoint_roundrobin_store();

        // Update RoundRobin store (uses its own data layer)
        for service_key in roundrobin_store.get_existing_service_keys() {
            roundrobin_store.update_lb_if_exists(&service_key);
        }

        // Update Consistent store (uses RoundRobin's data layer)
        let consistent_store = get_endpoint_consistent_store();
        for service_key in consistent_store.get_existing_service_keys() {
            consistent_store
                .update_lb_if_exists_with_provider(&service_key, |key| roundrobin_store.get_endpoint_for_service(key));
        }

        // Update LeastConn store (uses RoundRobin's data layer)
        let leastconn_store = get_endpoint_leastconn_store();
        for service_key in leastconn_store.get_existing_service_keys() {
            leastconn_store
                .update_lb_if_exists_with_provider(&service_key, |key| roundrobin_store.get_endpoint_for_service(key));
        }

        // Update EWMA store (uses RoundRobin's data layer)
        let ewma_store = get_endpoint_ewma_store();
        for service_key in ewma_store.get_existing_service_keys() {
            ewma_store
                .update_lb_if_exists_with_provider(&service_key, |key| roundrobin_store.get_endpoint_for_service(key));
        }
    }

    /// Update affected LBs in all stores (used after partial_update)
    fn update_affected_lbs(&self, affected_services: &HashSet<String>) {
        let roundrobin_store = get_endpoint_roundrobin_store();

        // Update RoundRobin store (uses its own data layer)
        for service_key in affected_services {
            roundrobin_store.update_lb_if_exists(service_key);
        }

        // Update Consistent store (uses RoundRobin's data layer)
        let consistent_store = get_endpoint_consistent_store();
        for service_key in affected_services {
            consistent_store
                .update_lb_if_exists_with_provider(service_key, |key| roundrobin_store.get_endpoint_for_service(key));
        }

        // Update LeastConn store (uses RoundRobin's data layer)
        let leastconn_store = get_endpoint_leastconn_store();
        for service_key in affected_services {
            leastconn_store
                .update_lb_if_exists_with_provider(service_key, |key| roundrobin_store.get_endpoint_for_service(key));
        }

        // Update EWMA store (uses RoundRobin's data layer)
        let ewma_store = get_endpoint_ewma_store();
        for service_key in affected_services {
            ewma_store
                .update_lb_if_exists_with_provider(service_key, |key| roundrobin_store.get_endpoint_for_service(key));
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

        // 4. Clear stale Endpoints-level config
        let stale_services: HashSet<String> = old_keys.difference(&all_services).cloned().collect();
        if !stale_services.is_empty() {
            for service_key in &stale_services {
                config_store.set_endpoint_config(service_key, None);
                get_health_check_manager().reconcile_service(service_key);
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
