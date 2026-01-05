use crate::core::conf_sync::traits::ConfHandler;
use crate::core::tls::rebuild_gateway_tls_matcher;
use crate::types::prelude_resources::Gateway;
use kube::ResourceExt;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

/// Global store for Gateway resources
static GATEWAY_STORE: std::sync::LazyLock<Arc<RwLock<Vec<Gateway>>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(Vec::new())));

/// Get a reference to the global Gateway store
#[allow(dead_code)]
pub fn get_gateway_store() -> Arc<RwLock<Vec<Gateway>>> {
    GATEWAY_STORE.clone()
}

/// Query Gateway by namespace and name
#[allow(dead_code)]
pub fn get_gateway_by_name(namespace: Option<&str>, name: &str) -> Option<Gateway> {
    let store = GATEWAY_STORE.read().unwrap();
    store
        .iter()
        .find(|gw| gw.name_any() == name && gw.namespace().as_deref() == namespace)
        .cloned()
}

/// List all Gateway resources
#[allow(dead_code)]
pub fn list_gateways() -> Vec<Gateway> {
    let store = GATEWAY_STORE.read().unwrap();
    store.clone()
}

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

        // Update global store
        let mut store = GATEWAY_STORE.write().unwrap();
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
            store.push(gateway.clone());
        }

        // Rebuild Gateway TLS matcher for dynamic TLS certificate lookup
        rebuild_gateway_tls_matcher(&store);
    }

    fn partial_update(&self, add: HashMap<String, Gateway>, update: HashMap<String, Gateway>, remove: HashSet<String>) {
        let mut store = GATEWAY_STORE.write().unwrap();

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
                store.push(gateway);
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

                // Find and update existing gateway
                if let Some(existing) = store
                    .iter_mut()
                    .find(|gw| gw.name_any() == gateway.name_any() && gw.namespace() == gateway.namespace())
                {
                    *existing = gateway;
                }

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
            }

            // Remove gateways whose key is in the remove set
            store.retain(|gw| {
                let gw_key = format!("{}/{}", gw.namespace().unwrap_or_default(), gw.name_any());
                !remove.contains(&gw_key) && !remove.contains(&gw.name_any())
            });

            // TODO: Clean up listeners (requires Pingora support or hot reload)
        }

        // Rebuild Gateway TLS matcher after any changes
        rebuild_gateway_tls_matcher(&store);
    }
}

/// Create Gateway handler
pub fn create_gateway_handler() -> Box<dyn ConfHandler<Gateway>> {
    Box::new(GatewayHandler::new())
}
