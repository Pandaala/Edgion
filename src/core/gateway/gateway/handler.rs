use crate::core::conf_sync::traits::ConfHandler;
use crate::core::gateway::gateway::get_global_gateway_store;
use crate::core::gateway::gateway::tls_matcher::rebuild_gateway_tls_matcher;
use crate::core::routes::http_routes::get_global_route_manager;
use crate::types::prelude_resources::Gateway;
use kube::ResourceExt;
use std::collections::{HashMap, HashSet};

/// ConfHandler implementation for Gateway
/// Stores Gateway resources in cache for dynamic lookup during bootstrap
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

        // Rebuild Gateway TLS matcher for dynamic TLS certificate lookup
        let gateways = store.list_gateways();
        rebuild_gateway_tls_matcher(&gateways);
    }

    fn partial_update(&self, add: HashMap<String, Gateway>, update: HashMap<String, Gateway>, remove: HashSet<String>) {
        let global_store = get_global_gateway_store();
        let mut store = global_store.write().unwrap();

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
                    "Gateway updated (dynamic listener/TLS update not yet implemented)"
                );
                store.update_gateway(gateway);

                // TODO: Detect listener changes
                // TODO: Detect TLS certificateRefs changes and update GatewayTlsCertMatcher
                // TODO: Detect hostname changes and update route matching
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
                    "Gateway removed (dynamic listener removal not yet implemented)"
                );
                if let Err(e) = store.remove_gateway(key) {
                    tracing::warn!(key = %key, error = %e, "Failed to remove gateway");
                }
            }

            // TODO: Clean up listeners (requires Pingora support or hot reload)
        }

        // Rebuild Gateway TLS matcher after any changes
        let gateways = store.list_gateways();
        rebuild_gateway_tls_matcher(&gateways);
    }
}

/// Create Gateway handler
pub fn create_gateway_handler() -> Box<dyn ConfHandler<Gateway>> {
    Box::new(GatewayHandler::new())
}
