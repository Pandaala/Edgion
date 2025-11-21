use std::collections::HashMap;
use crate::types::{EdgionGatewayConfig, Gateway, GatewayClass};

pub struct GatewayBaseConf {
    gateway_class: GatewayClass,
    edgion_gateway_config: EdgionGatewayConfig,
    gateways: Vec<Gateway>,
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
    
    fn make_gateway_key(gateway: &Gateway) -> String {
        if let Some(namespace) = &gateway.metadata.namespace {
            format!("{}/{}", namespace, gateway.metadata.name.as_deref().unwrap_or(""))
        } else {
            gateway.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
    
    pub fn gateway_class(&self) -> &GatewayClass {
        &self.gateway_class
    }
    
    pub fn edgion_gateway_config(&self) -> &EdgionGatewayConfig {
        &self.edgion_gateway_config
    }
    
    pub fn gateways(&self) -> &Vec<Gateway> {
        &self.gateways
    }
}