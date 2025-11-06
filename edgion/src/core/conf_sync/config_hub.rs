use crate::core::conf_sync::center_cache::{EventDispatch, ListData};
use crate::core::conf_sync::config_center::GatewayClassKey;
use crate::core::conf_sync::hub_cache::HubCache;
use crate::core::conf_sync::traits::EventDispatcher;
use crate::types::{EdgionTls, Gateway, GatewayClass, GatewayClassSpec, HTTPRoute, ResourceKind};
use anyhow::Result;
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

pub struct ConfigHub {
    gateway_class_key: GatewayClassKey,
    gateway_classes: HubCache<GatewayClass>,
    gateway_class_specs: HubCache<GatewayClassSpec>,
    gateways: HubCache<Gateway>,
    routes: HubCache<HTTPRoute>,
    services: HubCache<Service>,
    endpoint_slices: HubCache<EndpointSlice>,
    edgion_tls: HubCache<EdgionTls>,
    secrets: HubCache<Secret>,
}

impl ConfigHub {
    pub fn new(gateway_class_key: GatewayClassKey) -> Self {
        Self {
            gateway_class_key,
            gateway_classes: HubCache::new(),
            gateway_class_specs: HubCache::new(),
            gateways: HubCache::new(),
            routes: HubCache::new(),
            services: HubCache::new(),
            endpoint_slices: HubCache::new(),
            edgion_tls: HubCache::new(),
            secrets: HubCache::new(),
        }
    }

    pub fn get_gateway_class_key(&self) -> &GatewayClassKey {
        &self.gateway_class_key
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
            ResourceKind::GatewayClassSpec => {
                let list_data = self.gateway_class_specs.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize GatewayClassSpec data: {}", e))?;
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

    /// List gateway class specs
    pub fn list_gateway_class_specs(&self) -> ListData<&GatewayClassSpec> {
        self.gateway_class_specs.list()
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

        // Gateway Class Specs
        let list_data = self.list_gateway_class_specs();
        println!(
            "GatewayClassSpecs (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, spec) in list_data.data.iter().enumerate() {
            println!(
                "  [{}] {}",
                idx,
                serde_json::to_string(spec).unwrap_or_else(|_| "serialization error".to_string())
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

impl EventDispatcher for ConfigHub {
    fn init_add(
        &mut self,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            eprintln!(
                "[HUB] init_add: Failed to determine resource type from data: {}",
                &data[..data.len().min(200)]
            );
            return;
        };

        match resource_type {
            ResourceKind::GatewayClass => match serde_json::from_str::<GatewayClass>(&data) {
                Ok(resource) => self.gateway_classes.init_add(resource, resource_version),
                Err(e) => eprintln!(
                    "[HUB] init_add: Failed to parse GatewayClass: {} (data: {})",
                    e,
                    &data[..data.len().min(200)]
                ),
            },
            ResourceKind::GatewayClassSpec => {
                match serde_json::from_str::<GatewayClassSpec>(&data) {
                    Ok(resource) => self
                        .gateway_class_specs
                        .init_add(resource, resource_version),
                    Err(e) => eprintln!(
                        "[HUB] init_add: Failed to parse GatewayClassSpec: {} (data: {})",
                        e,
                        &data[..data.len().min(200)]
                    ),
                }
            }
            ResourceKind::Gateway => match serde_json::from_str::<Gateway>(&data) {
                Ok(resource) => self.gateways.init_add(resource, resource_version),
                Err(e) => eprintln!(
                    "[HUB] init_add: Failed to parse Gateway: {} (data: {})",
                    e,
                    &data[..data.len().min(200)]
                ),
            },
            ResourceKind::HTTPRoute => match serde_json::from_str::<HTTPRoute>(&data) {
                Ok(resource) => self.routes.init_add(resource, resource_version),
                Err(e) => eprintln!(
                    "[HUB] init_add: Failed to parse HTTPRoute: {} (data: {})",
                    e,
                    &data[..data.len().min(200)]
                ),
            },
            ResourceKind::Service => match serde_json::from_str::<Service>(&data) {
                Ok(resource) => self.services.init_add(resource, resource_version),
                Err(e) => eprintln!(
                    "[HUB] init_add: Failed to parse Service: {} (data: {})",
                    e,
                    &data[..data.len().min(200)]
                ),
            },
            ResourceKind::EndpointSlice => match serde_json::from_str::<EndpointSlice>(&data) {
                Ok(resource) => self.endpoint_slices.init_add(resource, resource_version),
                Err(e) => eprintln!(
                    "[HUB] init_add: Failed to parse EndpointSlice: {} (data: {})",
                    e,
                    &data[..data.len().min(200)]
                ),
            },
            ResourceKind::EdgionTls => match serde_json::from_str::<EdgionTls>(&data) {
                Ok(resource) => self.edgion_tls.init_add(resource, resource_version),
                Err(e) => eprintln!(
                    "[HUB] init_add: Failed to parse EdgionTls: {} (data: {})",
                    e,
                    &data[..data.len().min(200)]
                ),
            },
            ResourceKind::Secret => match serde_json::from_str::<Secret>(&data) {
                Ok(resource) => self.secrets.init_add(resource, resource_version),
                Err(e) => eprintln!(
                    "[HUB] init_add: Failed to parse Secret: {} (data: {})",
                    e,
                    &data[..data.len().min(200)]
                ),
            },
        }
    }

    fn set_ready(&mut self) {
        // HubCache doesn't need ready state
    }

    fn event_add(
        &mut self,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            return;
        };

        match resource_type {
            ResourceKind::GatewayClass => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    self.gateway_classes.event_add(resource, resource_version);
                }
            }
            ResourceKind::GatewayClassSpec => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    self.gateway_class_specs
                        .event_add(resource, resource_version);
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    self.gateways.event_add(resource, resource_version);
                }
            }
            ResourceKind::HTTPRoute => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    self.routes.event_add(resource, resource_version);
                }
            }
            ResourceKind::Service => {
                if let Ok(resource) = serde_json::from_str::<Service>(&data) {
                    self.services.event_add(resource, resource_version);
                }
            }
            ResourceKind::EndpointSlice => {
                if let Ok(resource) = serde_json::from_str::<EndpointSlice>(&data) {
                    self.endpoint_slices.event_add(resource, resource_version);
                }
            }
            ResourceKind::EdgionTls => {
                if let Ok(resource) = serde_json::from_str::<EdgionTls>(&data) {
                    self.edgion_tls.event_add(resource, resource_version);
                }
            }
            ResourceKind::Secret => {
                if let Ok(resource) = serde_json::from_str::<Secret>(&data) {
                    self.secrets.event_add(resource, resource_version);
                }
            }
        }
    }

    fn event_update(
        &mut self,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            return;
        };

        match resource_type {
            ResourceKind::GatewayClass => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    self.gateway_classes
                        .event_update(resource, resource_version);
                }
            }
            ResourceKind::GatewayClassSpec => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    self.gateway_class_specs
                        .event_update(resource, resource_version);
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    self.gateways.event_update(resource, resource_version);
                }
            }
            ResourceKind::HTTPRoute => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    self.routes.event_update(resource, resource_version);
                }
            }
            ResourceKind::Service => {
                if let Ok(resource) = serde_json::from_str::<Service>(&data) {
                    self.services.event_update(resource, resource_version);
                }
            }
            ResourceKind::EndpointSlice => {
                if let Ok(resource) = serde_json::from_str::<EndpointSlice>(&data) {
                    self.endpoint_slices
                        .event_update(resource, resource_version);
                }
            }
            ResourceKind::EdgionTls => {
                if let Ok(resource) = serde_json::from_str::<EdgionTls>(&data) {
                    self.edgion_tls.event_update(resource, resource_version);
                }
            }
            ResourceKind::Secret => {
                if let Ok(resource) = serde_json::from_str::<Secret>(&data) {
                    self.secrets.event_update(resource, resource_version);
                }
            }
        }
    }

    fn event_del(
        &mut self,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            return;
        };

        match resource_type {
            ResourceKind::GatewayClass => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    self.gateway_classes.event_del(resource, resource_version);
                }
            }
            ResourceKind::GatewayClassSpec => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    self.gateway_class_specs
                        .event_del(resource, resource_version);
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    self.gateways.event_del(resource, resource_version);
                }
            }
            ResourceKind::HTTPRoute => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    self.routes.event_del(resource, resource_version);
                }
            }
            ResourceKind::Service => {
                if let Ok(resource) = serde_json::from_str::<Service>(&data) {
                    self.services.event_del(resource, resource_version);
                }
            }
            ResourceKind::EndpointSlice => {
                if let Ok(resource) = serde_json::from_str::<EndpointSlice>(&data) {
                    self.endpoint_slices.event_del(resource, resource_version);
                }
            }
            ResourceKind::EdgionTls => {
                if let Ok(resource) = serde_json::from_str::<EdgionTls>(&data) {
                    self.edgion_tls.event_del(resource, resource_version);
                }
            }
            ResourceKind::Secret => {
                if let Ok(resource) = serde_json::from_str::<Secret>(&data) {
                    self.secrets.event_del(resource, resource_version);
                }
            }
        }
    }
}
