use crate::core::conf_sync::cache_server::{EventDispatch, ListData, Versionable};
use crate::core::conf_sync::config_server::GatewayClassKey;
use crate::core::conf_sync::cache_client::ClientCache;
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::types::{
    EdgionGatewayConfig, EdgionTls, Gateway, GatewayClass, HTTPRoute, ResourceKind,
};
use anyhow::Result;
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

pub struct ConfigClient {
    gateway_class_key: GatewayClassKey,
    gateway_classes: ClientCache<GatewayClass>,
    edgion_gateway_configs: ClientCache<EdgionGatewayConfig>,
    gateways: ClientCache<Gateway>,
    routes: ClientCache<HTTPRoute>,
    services: ClientCache<Service>,
    endpoint_slices: ClientCache<EndpointSlice>,
    edgion_tls: ClientCache<EdgionTls>,
    secrets: ClientCache<Secret>,
}

impl ConfigClient {
    pub fn new(gateway_class_key: GatewayClassKey) -> Self {
        Self {
            gateway_class_key,
            gateway_classes: ClientCache::new(),
            edgion_gateway_configs: ClientCache::new(),
            gateways: ClientCache::new(),
            routes: ClientCache::new(),
            services: ClientCache::new(),
            endpoint_slices: ClientCache::new(),
            edgion_tls: ClientCache::new(),
            secrets: ClientCache::new(),
        }
    }

    pub fn get_gateway_class_key(&self) -> &GatewayClassKey {
        &self.gateway_class_key
    }

    fn apply_change_to_cache<T>(
        cache: &mut ClientCache<T>,
        change: ResourceChange,
        resource: T,
        resource_version: Option<u64>,
    ) where
        T: Clone + Versionable + Send + 'static,
    {
        cache.apply_change(change, resource, resource_version);
    }

    pub fn list(
        &self,
        key: &GatewayClassKey,
        kind: &ResourceKind,
    ) -> Result<ListDataSimple, String> {
        if key != &self.gateway_class_key {
            return Err(format!(
                "Key mismatch: expected {}, got {}",
                self.gateway_class_key, key
            ));
        }

        let (data_json, resource_version) = match kind {
            ResourceKind::GatewayClass => {
                let list_data = self.gateway_classes.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize GatewayClass data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionGatewayConfig => {
                let list_data = self.edgion_gateway_configs.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionGatewayConfig data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Gateway => {
                let list_data = self.gateways.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Gateway data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::HTTPRoute => {
                let list_data = self.routes.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize HTTPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Service => {
                let list_data = self.services.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Service data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EndpointSlice => {
                let list_data = self.endpoint_slices.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EndpointSlice data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionTls => {
                let list_data = self.edgion_tls.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionTls data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Secret => {
                let list_data = self.secrets.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Secret data: {}", e))?;
                (json, list_data.resource_version)
            }
        };

        Ok(ListDataSimple {
            data: data_json,
            resource_version,
        })
    }

    /// List gateway classes
    pub fn list_gateway_classes(&self) -> ListData<&GatewayClass> {
        self.gateway_classes.list()
    }

    /// List gateway class configs
    pub fn list_edgion_gateway_config(&self) -> ListData<&EdgionGatewayConfig> {
        self.edgion_gateway_configs.list()
    }

    /// List gateways
    pub fn list_gateways(&self) -> ListData<&Gateway> {
        self.gateways.list()
    }

    /// List HTTP routes
    pub fn list_routes(&self) -> ListData<&HTTPRoute> {
        self.routes.list()
    }

    /// List services
    pub fn list_services(&self) -> ListData<&Service> {
        self.services.list()
    }

    /// List endpoint slices
    pub fn list_endpoint_slices(&self) -> ListData<&EndpointSlice> {
        self.endpoint_slices.list()
    }

    /// List Edgion TLS
    pub fn list_edgion_tls(&self) -> ListData<&EdgionTls> {
        self.edgion_tls.list()
    }

    /// List secrets
    pub fn list_secrets(&self) -> ListData<&Secret> {
        self.secrets.list()
    }

    /// Print all configuration for the gateway class key
    /// Format is identical to ConfigCenter::print_config
    pub fn print_config(&self) {
        let key = &self.gateway_class_key;
        println!("=== ConfigHub Config for GatewayClassKey: {} ===", key);

        // Gateway Classes
        let list_data = self.list_gateway_classes();
        println!(
            "GatewayClasses (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, gc) in list_data.data.iter().enumerate() {
            println!(
                "  [{}] {}",
                idx,
                serde_json::to_string(gc).unwrap_or_else(|_| "serialization error".to_string())
            );
        }

        let list_data = self.list_edgion_gateway_config();
        println!(
            "EdgionGatewayConfigs (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, egw) in list_data.data.iter().enumerate() {
            println!(
                "  [{}] {}",
                idx,
                serde_json::to_string(egw).unwrap_or_else(|_| "serialization error".to_string())
            );
        }

        // Gateways
        let list_data = self.list_gateways();
        println!(
            "Gateways (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, gw) in list_data.data.iter().enumerate() {
            println!(
                "  [{}] {}",
                idx,
                serde_json::to_string(gw).unwrap_or_else(|_| "serialization error".to_string())
            );
        }

        // HTTP Routes
        let list_data = self.list_routes();
        println!(
            "HTTPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!(
                "  [{}] {}",
                idx,
                serde_json::to_string(route).unwrap_or_else(|_| "serialization error".to_string())
            );
        }

        // Services
        let list_data = self.list_services();
        println!(
            "Services (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, svc) in list_data.data.iter().enumerate() {
            println!(
                "  [{}] {}",
                idx,
                serde_json::to_string(svc).unwrap_or_else(|_| "serialization error".to_string())
            );
        }

        // Endpoint Slices
        let list_data = self.list_endpoint_slices();
        println!(
            "EndpointSlices (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, es) in list_data.data.iter().enumerate() {
            println!(
                "  [{}] {}",
                idx,
                serde_json::to_string(es).unwrap_or_else(|_| "serialization error".to_string())
            );
        }

        // Edgion TLS
        let list_data = self.list_edgion_tls();
        println!(
            "EdgionTls (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, tls) in list_data.data.iter().enumerate() {
            println!(
                "  [{}] {}",
                idx,
                serde_json::to_string(tls).unwrap_or_else(|_| "serialization error".to_string())
            );
        }

        // Secrets
        let list_data = self.list_secrets();
        println!(
            "Secrets (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, secret) in list_data.data.iter().enumerate() {
            println!(
                "  [{}] {}",
                idx,
                serde_json::to_string(secret).unwrap_or_else(|_| "serialization error".to_string())
            );
        }

        println!("=== End ConfigHub Config ===\n");
    }
}

pub struct ListDataSimple {
    pub data: String,
    pub resource_version: u64,
}

impl EventDispatcher for ConfigClient {
    fn apply_resource_change(
        &mut self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            eprintln!(
                "[HUB] apply_resource_change {:?}: Failed to determine resource type from data: {}",
                change,
                &data[..data.len().min(200)]
            );
            return;
        };

        let log_error = |kind: &str, err: &serde_json::Error| {
            eprintln!(
                "[HUB] apply_resource_change {:?}: Failed to parse {}: {} (data: {})",
                change,
                kind,
                err,
                &data[..data.len().min(200)]
            );
        };

        match resource_type {
            ResourceKind::GatewayClass => match serde_json::from_str::<GatewayClass>(&data) {
                Ok(resource) => Self::apply_change_to_cache(
                    &mut self.gateway_classes,
                    change,
                    resource,
                    resource_version,
                ),
                Err(e) => log_error("GatewayClass", &e),
            },
            ResourceKind::EdgionGatewayConfig => {
                match serde_json::from_str::<EdgionGatewayConfig>(&data) {
                    Ok(resource) => Self::apply_change_to_cache(
                        &mut self.edgion_gateway_configs,
                        change,
                        resource,
                        resource_version,
                    ),
                    Err(e) => log_error("EdgionGatewayConfig", &e),
                }
            }
            ResourceKind::Gateway => match serde_json::from_str::<Gateway>(&data) {
                Ok(resource) => Self::apply_change_to_cache(
                    &mut self.gateways,
                    change,
                    resource,
                    resource_version,
                ),
                Err(e) => log_error("Gateway", &e),
            },
            ResourceKind::HTTPRoute => match serde_json::from_str::<HTTPRoute>(&data) {
                Ok(resource) => Self::apply_change_to_cache(
                    &mut self.routes,
                    change,
                    resource,
                    resource_version,
                ),
                Err(e) => log_error("HTTPRoute", &e),
            },
            ResourceKind::Service => match serde_json::from_str::<Service>(&data) {
                Ok(resource) => Self::apply_change_to_cache(
                    &mut self.services,
                    change,
                    resource,
                    resource_version,
                ),
                Err(e) => log_error("Service", &e),
            },
            ResourceKind::EndpointSlice => match serde_json::from_str::<EndpointSlice>(&data) {
                Ok(resource) => Self::apply_change_to_cache(
                    &mut self.endpoint_slices,
                    change,
                    resource,
                    resource_version,
                ),
                Err(e) => log_error("EndpointSlice", &e),
            },
            ResourceKind::EdgionTls => match serde_json::from_str::<EdgionTls>(&data) {
                Ok(resource) => Self::apply_change_to_cache(
                    &mut self.edgion_tls,
                    change,
                    resource,
                    resource_version,
                ),
                Err(e) => log_error("EdgionTls", &e),
            },
            ResourceKind::Secret => match serde_json::from_str::<Secret>(&data) {
                Ok(resource) => Self::apply_change_to_cache(
                    &mut self.secrets,
                    change,
                    resource,
                    resource_version,
                ),
                Err(e) => log_error("Secret", &e),
            },
        }
    }

    fn set_ready(&mut self) {
        // HubCache doesn't need ready state
    }
}
