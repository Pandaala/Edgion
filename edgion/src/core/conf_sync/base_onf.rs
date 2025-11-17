use std::collections::HashMap;
use crate::types::{EdgionGatewayConfig, Gateway, GatewayClass};

pub struct GatewayClassBaseConf {
    gateway_class: Option<GatewayClass>,
    edgion_gateway_config: Option<EdgionGatewayConfig>,
    gateways: Vec<Gateway>,
    gateway_map: HashMap<String, ()>,
}

impl GatewayClassBaseConf {
    pub fn new() -> GatewayClassBaseConf {
        Self {
            gateway_class: None,
            edgion_gateway_config: None,
            gateways: Vec::new(),
            gateway_map: HashMap::new(),
        }
    }

    pub fn set_gateway_class(&mut self, gateway_class: GatewayClass) {
        self.gateway_class = Some(gateway_class);
    }

    pub fn set_edgion_gateway_config(&mut self, edgion_gateway_config: EdgionGatewayConfig) {
        self.edgion_gateway_config = Some(edgion_gateway_config);
    }

    pub fn add_gateway(&mut self, gateway: Gateway) {
        // Generate a key for the gateway (namespace/name)
        let key = if let Some(namespace) = &gateway.metadata.namespace {
            format!("{}/{}", namespace, gateway.metadata.name.as_deref().unwrap_or(""))
        } else {
            gateway.metadata.name.as_deref().unwrap_or("").to_string()
        };
        
        // Check if gateway already exists
        if !self.gateway_map.contains_key(&key) {
            self.gateway_map.insert(key.clone(), ());
            self.gateways.push(gateway);
        } else {
            // Update existing gateway
            if let Some(existing) = self.gateways.iter_mut().find(|g| {
                let existing_key = if let Some(ns) = &g.metadata.namespace {
                    format!("{}/{}", ns, g.metadata.name.as_deref().unwrap_or(""))
                } else {
                    g.metadata.name.as_deref().unwrap_or("").to_string()
                };
                existing_key == key
            }) {
                *existing = gateway;
            }
        }
    }

    pub fn remove_gateway(&mut self, namespace: Option<&String>, name: Option<&String>) {
        let key = if let Some(ns) = namespace {
            format!("{}/{}", ns, name.unwrap_or(&"".to_string()))
        } else {
            name.unwrap_or(&"".to_string()).clone()
        };
        
        self.gateway_map.remove(&key);
        self.gateways.retain(|g| {
            let existing_key = if let Some(ns) = &g.metadata.namespace {
                format!("{}/{}", ns, g.metadata.name.as_deref().unwrap_or(""))
            } else {
                g.metadata.name.as_deref().unwrap_or("").to_string()
            };
            existing_key != key
        });
    }

    pub fn clear_gateway_class(&mut self) {
        self.gateway_class = None;
    }

    pub fn clear_edgion_gateway_config(&mut self) {
        self.edgion_gateway_config = None;
    }

    pub fn gateway_class(&self) -> Option<&GatewayClass> {
        self.gateway_class.as_ref()
    }

    pub fn edgion_gateway_config(&self) -> Option<&EdgionGatewayConfig> {
        self.edgion_gateway_config.as_ref()
    }

    pub fn gateways(&self) -> &Vec<Gateway> {
        &self.gateways
    }
}