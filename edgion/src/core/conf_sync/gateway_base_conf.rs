use std::collections::HashMap;
use crate::types::{EdgionGatewayConfig, Gateway, GatewayClass};
use serde::{Deserialize, Serialize};
use anyhow::{Result, anyhow};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayBaseConf {
    gateway_class: GatewayClass,
    edgion_gateway_config: EdgionGatewayConfig,
    gateways: Vec<Gateway>,
    #[serde(skip)]
    gateway_map: HashMap<String, ()>,
}

impl GatewayBaseConf {
    pub fn new(
        gateway_class: GatewayClass,
        edgion_gateway_config: EdgionGatewayConfig,
        gateways: Vec<Gateway>,
    ) -> Self {
        let mut gateway_map = HashMap::new();
        for gateway in &gateways {
            let key = Self::make_gateway_key(gateway);
            gateway_map.insert(key, ());
        }
        
        Self {
            gateway_class,
            edgion_gateway_config,
            gateways,
            gateway_map,
        }
    }
    
    /// Rebuild gateway_map from gateways (used after deserialization)
    pub fn rebuild_gateway_map(&mut self) {
        self.gateway_map.clear();
        for gateway in &self.gateways {
            let key = Self::make_gateway_key(gateway);
            self.gateway_map.insert(key, ());
        }
    }
    
    fn make_gateway_key(gateway: &Gateway) -> String {
        if let Some(namespace) = &gateway.metadata.namespace {
            format!("{}/{}", namespace, gateway.metadata.name.as_deref().unwrap_or(""))
        } else {
            gateway.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
    
    fn make_gateway_key_from_parts(namespace: Option<&String>, name: Option<&String>) -> String {
        if let Some(ns) = namespace {
            format!("{}/{}", ns, name.unwrap_or(&"".to_string()))
        } else {
            name.unwrap_or(&"".to_string()).clone()
        }
    }
    
    pub fn gateway_class(&self) -> &GatewayClass {
        &self.gateway_class
    }
    
    /// Get the gateway class name
    pub fn gateway_class_name(&self) -> Option<&String> {
        self.gateway_class.metadata.name.as_ref()
    }
    
    pub fn edgion_gateway_config(&self) -> &EdgionGatewayConfig {
        &self.edgion_gateway_config
    }
    
    pub fn gateways(&self) -> &Vec<Gateway> {
        &self.gateways
    }
    
    /// Add a new gateway or update an existing one
    pub fn add_gateway(&mut self, gateway: Gateway) {
        let key = Self::make_gateway_key(&gateway);
        
        // Check if gateway already exists
        if !self.gateway_map.contains_key(&key) {
            self.gateway_map.insert(key.clone(), ());
            self.gateways.push(gateway);
        } else {
            // Update existing gateway
            if let Some(existing) = self.gateways.iter_mut().find(|g| {
                Self::make_gateway_key(g) == key
            }) {
                *existing = gateway;
            }
        }
    }
    
    /// Remove a gateway by namespace and name
    pub fn remove_gateway(&mut self, namespace: Option<&String>, name: Option<&String>) {
        let key = Self::make_gateway_key_from_parts(namespace, name);
        
        self.gateway_map.remove(&key);
        self.gateways.retain(|g| {
            Self::make_gateway_key(g) != key
        });
    }
    
    /// Check if a gateway exists
    pub fn has_gateway(&self, namespace: Option<&String>, name: Option<&String>) -> bool {
        let key = Self::make_gateway_key_from_parts(namespace, name);
        self.gateway_map.contains_key(&key)
    }
    
    /// Set gateway class
    pub fn set_gateway_class(&mut self, gateway_class: GatewayClass) {
        self.gateway_class = gateway_class;
    }
    
    /// Set edgion gateway config
    pub fn set_edgion_gateway_config(&mut self, edgion_gateway_config: EdgionGatewayConfig) {
        self.edgion_gateway_config = edgion_gateway_config;
    }
    
    /// Validate base configuration schema
    /// Returns an error if the configuration doesn't meet basic requirements
    pub fn validate_schema(&self) -> Result<()> {
        // Check that at least one gateway exists
        if self.gateways.is_empty() {
            return Err(anyhow!(
                "Base configuration validation failed: at least one Gateway must be defined"
            ));
        }
        
        tracing::info!(
            component = "gateway_base_conf",
            event = "schema_validation_passed",
            gateway_count = self.gateways.len(),
            "Base configuration schema validation passed"
        );
        
        Ok(())
    }
}