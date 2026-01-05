use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::types::{Gateway, ResourceMeta};
use anyhow::{anyhow, Result};

pub struct GatewayStore {
    gateways: HashMap<String, Gateway>,
}

impl GatewayStore {
    pub fn new() -> Self {
        GatewayStore {
            gateways: HashMap::new(),
        }
    }

    pub fn add_gateway(&mut self, gateway: Gateway) -> Result<()> {
        let key = gateway.key_name();
        if self.gateways.contains_key(&key) {
            return Err(anyhow!("Gateway with key '{}' already exists in store", key));
        }
        self.gateways.insert(key, gateway);
        Ok(())
    }

    pub fn get_gateway(&self, key: &str) -> Result<&Gateway> {
        self.gateways
            .get(key)
            .ok_or_else(|| anyhow!("Gateway with key '{}' not found in store", key))
    }

    #[allow(dead_code)]
    pub fn remove_gateway(&mut self, key: &str) -> Result<()> {
        if !self.gateways.contains_key(key) {
            return Err(anyhow!("Gateway with key '{}' not found in store", key));
        }
        self.gateways.remove(key);
        Ok(())
    }
}

/// Global GatewayStore instance
static GLOBAL_GATEWAY_STORE: std::sync::LazyLock<Arc<RwLock<GatewayStore>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(GatewayStore::new())));

/// Get the global GatewayStore instance
pub fn get_global_gateway_store() -> Arc<RwLock<GatewayStore>> {
    GLOBAL_GATEWAY_STORE.clone()
}
