use crate::core::conf_sync::base_onf::GatewayClassBaseConf;
use crate::core::conf_sync::cache_client::ClientCache;
use crate::core::conf_sync::cache_server::{EventDispatch, ListData, Versionable};
use crate::core::conf_sync::config_server::GatewayClassKey;
use crate::core::conf_sync::traits::{ConfigClientEventDispatcher, ResourceChange};
use crate::core::utils::format_resource_info;
use crate::types::{EdgionGatewayConfig, EdgionTls, Gateway, GatewayClass, HTTPRoute, ResourceKind};
use anyhow::Result;
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;
use std::sync::RwLock;

pub struct ConfigClient {
    gateway_class_key: GatewayClassKey,
    base_conf: RwLock<GatewayClassBaseConf>,
    routes: ClientCache<HTTPRoute>,
    services: ClientCache<Service>,
    endpoint_slices: ClientCache<EndpointSlice>,
    edgion_tls: ClientCache<EdgionTls>,
    secrets: ClientCache<Secret>,
}

impl ConfigClient {
    pub fn new(gateway_class_key: GatewayClassKey, client_id: String, client_name: String) -> Self {
        Self {
            gateway_class_key: gateway_class_key.clone(),
            base_conf: RwLock::new(GatewayClassBaseConf::new()),
            routes: ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone()),
            services: ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone()),
            endpoint_slices: ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone()),
            edgion_tls: ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone()),
            secrets: ClientCache::new(gateway_class_key, client_id, client_name),
        }
    }

    /// Get routes cache for direct access
    pub fn routes(&self) -> &ClientCache<HTTPRoute> {
        &self.routes
    }

    /// Get services cache for direct access
    pub fn services(&self) -> &ClientCache<Service> {
        &self.services
    }

    /// Get endpoint_slices cache for direct access
    pub fn endpoint_slices(&self) -> &ClientCache<EndpointSlice> {
        &self.endpoint_slices
    }

    /// Get edgion_tls cache for direct access
    pub fn edgion_tls(&self) -> &ClientCache<EdgionTls> {
        &self.edgion_tls
    }

    /// Get secrets cache for direct access
    pub fn secrets(&self) -> &ClientCache<Secret> {
        &self.secrets
    }

    pub fn get_gateway_class_key(&self) -> &GatewayClassKey {
        &self.gateway_class_key
    }

    /// Initialize base configuration with parsed objects
    pub fn init_base_conf(&self, new_base_conf: GatewayClassBaseConf) {
        let mut base_conf = self.base_conf.write().unwrap();
        *base_conf = new_base_conf;
    }

    fn apply_change_to_cache<T>(cache: &ClientCache<T>, change: ResourceChange, resource: T)
    where
        T: Clone + Versionable + Resource + Send + 'static,
    {
        cache.apply_change(change, resource);
    }

    pub fn list(&self, key: &GatewayClassKey, kind: &ResourceKind) -> Result<ListDataSimple, String> {
        if key != &self.gateway_class_key {
            return Err(format!(
                "Key mismatch: expected {}, got {}",
                self.gateway_class_key, key
            ));
        }

        let (data_json, resource_version) = match kind {
            ResourceKind::Unspecified => {
                return Err("Resource kind unspecified".to_string());
            }
            ResourceKind::GatewayClass => {
                let base_conf = self.base_conf.read().unwrap();
                let data: Vec<GatewayClass> = base_conf.gateway_class().map(|gc| vec![gc.clone()]).unwrap_or_default();
                let json = serde_json::to_string(&data)
                    .map_err(|e| format!("Failed to serialize GatewayClass data: {}", e))?;
                // Base conf resources don't have version tracking, use 0
                (json, 0)
            }
            ResourceKind::EdgionGatewayConfig => {
                let base_conf = self.base_conf.read().unwrap();
                let data: Vec<EdgionGatewayConfig> = base_conf
                    .edgion_gateway_config()
                    .map(|egwc| vec![egwc.clone()])
                    .unwrap_or_default();
                let json = serde_json::to_string(&data)
                    .map_err(|e| format!("Failed to serialize EdgionGatewayConfig data: {}", e))?;
                // Base conf resources don't have version tracking, use 0
                (json, 0)
            }
            ResourceKind::Gateway => {
                let base_conf = self.base_conf.read().unwrap();
                let data = base_conf.gateways().clone();
                let json =
                    serde_json::to_string(&data).map_err(|e| format!("Failed to serialize Gateway data: {}", e))?;
                // Base conf resources don't have version tracking, use 0
                (json, 0)
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

    /// List HTTP routes
    pub fn list_routes(&self) -> ListData<HTTPRoute> {
        self.routes.list_owned()
    }

    /// List services
    pub fn list_services(&self) -> ListData<Service> {
        self.services.list_owned()
    }

    /// List endpoint slices
    pub fn list_endpoint_slices(&self) -> ListData<EndpointSlice> {
        self.endpoint_slices.list_owned()
    }

    /// List Edgion TLS
    pub fn list_edgion_tls(&self) -> ListData<EdgionTls> {
        self.edgion_tls.list_owned()
    }

    /// List secrets
    pub fn list_secrets(&self) -> ListData<Secret> {
        self.secrets.list_owned()
    }

    /// Print all configuration for the gateway class key
    /// Format is identical to ConfigCenter::print_config
    pub fn print_config(&self) {
        let key = &self.gateway_class_key;
        println!("=== ConfigHub Config for GatewayClassKey: {} ===", key);

        // Base conf resources are stored in base_conf
        let base_conf = self.base_conf.read().unwrap();
        if let Some(gc) = base_conf.gateway_class() {
            println!("GatewayClass:");
            println!("  [0] {}", format_resource_info(gc));
        } else {
            println!("GatewayClass: not found");
        }

        if let Some(egwc) = base_conf.edgion_gateway_config() {
            println!("EdgionGatewayConfig:");
            println!("  [0] {}", format_resource_info(egwc));
        } else {
            println!("EdgionGatewayConfig: not found");
        }

        let gateways = base_conf.gateways();
        if !gateways.is_empty() {
            println!("Gateways (count: {}):", gateways.len());
            for (idx, gw) in gateways.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(gw));
            }
        } else {
            println!("Gateways: not found");
        }
        drop(base_conf);

        // HTTP Routes
        let list_data = self.list_routes();
        println!(
            "HTTPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // Services
        let list_data = self.list_services();
        println!(
            "Services (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, svc) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(svc));
        }

        // Endpoint Slices
        let list_data = self.list_endpoint_slices();
        println!(
            "EndpointSlices (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, es) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(es));
        }

        // Edgion TLS
        let list_data = self.list_edgion_tls();
        println!(
            "EdgionTls (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, tls) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(tls));
        }

        // Secrets
        let list_data = self.list_secrets();
        println!(
            "Secrets (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, secret) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(secret));
        }

        println!("=== End ConfigHub Config ===\n");
    }
}

pub struct ListDataSimple {
    pub data: String,
    pub resource_version: u64,
}

impl ConfigClientEventDispatcher for ConfigClient {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        _resource_version: Option<u64>,
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

        let log_error = |kind: &str, err: &serde_yaml::Error| {
            eprintln!(
                "[HUB] apply_resource_change {:?}: Failed to parse {}: {} (data: {})",
                change,
                kind,
                err,
                &data[..data.len().min(200)]
            );
        };

        match resource_type {
            ResourceKind::Unspecified => {
                eprintln!(
                    "[HUB] apply_resource_change {:?}: Unspecified resource kind, skipping (data: {})",
                    change,
                    &data[..data.len().min(200)]
                );
            }
            ResourceKind::GatewayClass => match serde_yaml::from_str::<GatewayClass>(&data) {
                Ok(resource) => {
                    let mut base_conf = self.base_conf.write().unwrap();
                    match change {
                        ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                            base_conf.set_gateway_class(resource);
                        }
                        ResourceChange::EventDelete => {
                            base_conf.clear_gateway_class();
                        }
                    }
                }
                Err(e) => log_error("GatewayClass", &e),
            },
            ResourceKind::EdgionGatewayConfig => match serde_yaml::from_str::<EdgionGatewayConfig>(&data) {
                Ok(resource) => {
                    let mut base_conf = self.base_conf.write().unwrap();
                    match change {
                        ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                            base_conf.set_edgion_gateway_config(resource);
                        }
                        ResourceChange::EventDelete => {
                            base_conf.clear_edgion_gateway_config();
                        }
                    }
                }
                Err(e) => log_error("EdgionGatewayConfig", &e),
            },
            ResourceKind::Gateway => match serde_yaml::from_str::<Gateway>(&data) {
                Ok(resource) => {
                    let mut base_conf = self.base_conf.write().unwrap();
                    match change {
                        ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                            base_conf.add_gateway(resource);
                        }
                        ResourceChange::EventDelete => {
                            // For delete, we need to extract namespace and name before moving resource
                            let namespace = resource.metadata.namespace.clone();
                            let name = resource.metadata.name.clone();
                            base_conf.remove_gateway(namespace.as_ref(), name.as_ref());
                        }
                    }
                }
                Err(e) => log_error("Gateway", &e),
            },
            ResourceKind::HTTPRoute => match serde_yaml::from_str::<HTTPRoute>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.routes, change, resource);
                }
                Err(e) => log_error("HTTPRoute", &e),
            },
            ResourceKind::Service => match serde_yaml::from_str::<Service>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.services, change, resource);
                }
                Err(e) => log_error("Service", &e),
            },
            ResourceKind::EndpointSlice => match serde_yaml::from_str::<EndpointSlice>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.endpoint_slices, change, resource);
                }
                Err(e) => log_error("EndpointSlice", &e),
            },
            ResourceKind::EdgionTls => match serde_yaml::from_str::<EdgionTls>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.edgion_tls, change, resource);
                }
                Err(e) => log_error("EdgionTls", &e),
            },
            ResourceKind::Secret => match serde_yaml::from_str::<Secret>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.secrets, change, resource);
                }
                Err(e) => log_error("Secret", &e),
            },
        }
    }

    fn apply_base_conf(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        // ConfigClient also handles base_conf through apply_resource_change
        // This method is kept for trait compatibility but delegates to apply_resource_change
        self.apply_resource_change(change, resource_type, data, resource_version);
    }
}
