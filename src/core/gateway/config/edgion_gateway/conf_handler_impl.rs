use crate::core::common::conf_sync::traits::ConfHandler;
use crate::types::prelude_resources::EdgionGatewayConfig;
use kube::ResourceExt;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

/// Global store for EdgionGatewayConfig resources
static EDGION_GATEWAY_CONFIG_STORE: std::sync::LazyLock<Arc<RwLock<Vec<EdgionGatewayConfig>>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(Vec::new())));

/// Get a reference to the global EdgionGatewayConfig store
#[allow(dead_code)]
pub fn get_edgion_gateway_config_store() -> Arc<RwLock<Vec<EdgionGatewayConfig>>> {
    EDGION_GATEWAY_CONFIG_STORE.clone()
}

/// Query EdgionGatewayConfig by name
#[allow(dead_code)]
pub fn get_edgion_gateway_config_by_name(name: &str) -> Option<EdgionGatewayConfig> {
    let store = EDGION_GATEWAY_CONFIG_STORE.read().unwrap();
    store.iter().find(|config| config.name_any() == name).cloned()
}

/// List all EdgionGatewayConfig resources
#[allow(dead_code)]
pub fn list_edgion_gateway_configs() -> Vec<EdgionGatewayConfig> {
    let store = EDGION_GATEWAY_CONFIG_STORE.read().unwrap();
    store.clone()
}

/// ConfHandler implementation for EdgionGatewayConfig
/// Stores EdgionGatewayConfig resources in cache for dynamic lookup
pub struct EdgionGatewayConfigHandler;

impl EdgionGatewayConfigHandler {
    pub fn new() -> Self {
        Self
    }
}

impl ConfHandler<EdgionGatewayConfig> for EdgionGatewayConfigHandler {
    fn full_set(&self, data: &HashMap<String, EdgionGatewayConfig>) {
        tracing::info!(
            count = data.len(),
            "EdgionGatewayConfig full_set: received {} EdgionGatewayConfig resources",
            data.len()
        );

        // Update global store
        let mut store = EDGION_GATEWAY_CONFIG_STORE.write().unwrap();
        store.clear();
        for (key, config) in data {
            tracing::debug!(
                key = %key,
                name = %config.name_any(),
                "EdgionGatewayConfig stored"
            );
            store.push(config.clone());
        }
    }

    fn partial_update(
        &self,
        add: HashMap<String, EdgionGatewayConfig>,
        update: HashMap<String, EdgionGatewayConfig>,
        remove: HashSet<String>,
    ) {
        let mut store = EDGION_GATEWAY_CONFIG_STORE.write().unwrap();

        if !add.is_empty() {
            tracing::info!(
                count = add.len(),
                "EdgionGatewayConfig partial_update: added {} EdgionGatewayConfig resources",
                add.len()
            );
            for (key, config) in add {
                tracing::info!(
                    key = %key,
                    name = %config.name_any(),
                    "EdgionGatewayConfig added"
                );
                store.push(config);
            }
        }

        if !update.is_empty() {
            tracing::info!(
                count = update.len(),
                "EdgionGatewayConfig partial_update: updated {} EdgionGatewayConfig resources",
                update.len()
            );
            for (key, config) in update {
                tracing::info!(
                    key = %key,
                    name = %config.name_any(),
                    "EdgionGatewayConfig updated"
                );
                // Find and update existing config
                if let Some(existing) = store.iter_mut().find(|c| c.name_any() == config.name_any()) {
                    *existing = config;
                }
            }
        }

        if !remove.is_empty() {
            tracing::info!(
                count = remove.len(),
                "EdgionGatewayConfig partial_update: removed {} EdgionGatewayConfig resources",
                remove.len()
            );
            for key in &remove {
                tracing::info!(
                    key = %key,
                    "EdgionGatewayConfig removed"
                );
            }
            // Remove configs whose key is in the remove set
            store.retain(|config| !remove.contains(&config.name_any()));
        }
    }
}

/// Create EdgionGatewayConfig handler
pub fn create_edgion_gateway_config_handler() -> Box<dyn ConfHandler<EdgionGatewayConfig>> {
    Box::new(EdgionGatewayConfigHandler::new())
}
