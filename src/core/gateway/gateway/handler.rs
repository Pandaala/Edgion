use crate::core::conf_sync::traits::ConfHandler;
use crate::core::gateway::gateway::config_store::get_global_gateway_config_store;
use crate::core::gateway::gateway::get_global_gateway_store;
use crate::core::gateway::gateway::tls_matcher::rebuild_gateway_tls_matcher;
use crate::core::routes::http_routes::get_global_route_manager;
use crate::types::prelude_resources::Gateway;
use kube::ResourceExt;
use std::collections::{HashMap, HashSet};

/// ConfHandler implementation for Gateway
/// Stores Gateway resources in cache for dynamic lookup during bootstrap
///
/// This handler manages three stores:
/// 1. GatewayStore: Raw Gateway resources
/// 2. GatewayConfigStore: Two-layer structure (host_map + listener_map) for dynamic lookup
/// 3. GatewayTlsMatcher: Port-based TLS certificate matching
pub struct GatewayHandler;

impl GatewayHandler {
    pub fn new() -> Self {
        Self
    }
}

impl ConfHandler<Gateway> for GatewayHandler {
    fn full_set(&self, data: &HashMap<String, Gateway>) {
        tracing::info!(
            count = data.len(),
            "Gateway full_set: received {} Gateway resources",
            data.len()
        );

        let global_store = get_global_gateway_store();
        let mut store = global_store.write().unwrap();
        store.clear();

        for (key, gateway) in data {
            let listener_count = gateway.spec.listeners.as_ref().map(|l| l.len()).unwrap_or(0);
            tracing::info!(
                key = %key,
                namespace = ?gateway.namespace(),
                name = %gateway.name_any(),
                gateway_class = %gateway.spec.gateway_class_name,
                listeners = listener_count,
                "Gateway stored"
            );

            if let Err(e) = store.add_gateway(gateway.clone()) {
                tracing::warn!(key = %key, error = %e, "Failed to add gateway to store");
            }

            // Initialize route manager entry for this gateway
            let route_manager = get_global_route_manager();
            let namespace = gateway.namespace().unwrap_or_default();
            let name = gateway.name_any();
            route_manager.get_or_create_domain_routes(&namespace, &name);
        }

        // Get all gateways for rebuilding
        let gateways = store.list_gateways();

        // Rebuild GatewayConfigStore (two-layer structure for dynamic lookup)
        let config_store = get_global_gateway_config_store();
        config_store.full_set(&gateways);

        // Rebuild Gateway TLS matcher (port-based certificate lookup)
        rebuild_gateway_tls_matcher(&gateways);
    }

    fn partial_update(&self, add: HashMap<String, Gateway>, update: HashMap<String, Gateway>, remove: HashSet<String>) {
        let global_store = get_global_gateway_store();
        let mut store = global_store.write().unwrap();
        let config_store = get_global_gateway_config_store();

        if !add.is_empty() {
            tracing::info!(
                count = add.len(),
                "Gateway partial_update: added {} Gateway resources",
                add.len()
            );
            for (key, gateway) in add {
                let listener_count = gateway.spec.listeners.as_ref().map(|l| l.len()).unwrap_or(0);
                tracing::info!(
                    key = %key,
                    namespace = ?gateway.namespace(),
                    name = %gateway.name_any(),
                    gateway_class = %gateway.spec.gateway_class_name,
                    listeners = listener_count,
                    "Gateway added"
                );

                // Update GatewayConfigStore for dynamic lookup
                config_store.update_gateway(&gateway);

                if let Err(e) = store.add_gateway(gateway) {
                    tracing::warn!(key = %key, error = %e, "Failed to add gateway");
                }
            }
        }

        if !update.is_empty() {
            tracing::info!(
                count = update.len(),
                "Gateway partial_update: updated {} Gateway resources",
                update.len()
            );
            for (key, gateway) in update {
                let listener_count = gateway.spec.listeners.as_ref().map(|l| l.len()).unwrap_or(0);
                tracing::info!(
                    key = %key,
                    namespace = ?gateway.namespace(),
                    name = %gateway.name_any(),
                    gateway_class = %gateway.spec.gateway_class_name,
                    listeners = listener_count,
                    "Gateway updated (dynamic listener/TLS config updated)"
                );

                // Update GatewayConfigStore for dynamic lookup
                config_store.update_gateway(&gateway);

                store.update_gateway(gateway);
            }
        }

        if !remove.is_empty() {
            tracing::info!(
                count = remove.len(),
                "Gateway partial_update: removed {} Gateway resources",
                remove.len()
            );
            for key in &remove {
                tracing::info!(
                    key = %key,
                    "Gateway removed"
                );

                // Parse key to get namespace and name
                let parts: Vec<&str> = key.split('/').collect();
                let (namespace, name) = if parts.len() == 2 {
                    (parts[0], parts[1])
                } else {
                    ("", key.as_str())
                };

                // Remove from GatewayConfigStore
                config_store.remove_gateway(namespace, name);

                if let Err(e) = store.remove_gateway(key) {
                    tracing::warn!(key = %key, error = %e, "Failed to remove gateway");
                }
            }

            // Note: Physical listener cleanup requires Pingora restart
        }

        // Rebuild Gateway TLS matcher (port-based certificate lookup)
        let gateways = store.list_gateways();
        rebuild_gateway_tls_matcher(&gateways);
    }
}

/// Create Gateway handler
pub fn create_gateway_handler() -> Box<dyn ConfHandler<Gateway>> {
    Box::new(GatewayHandler::new())
}
