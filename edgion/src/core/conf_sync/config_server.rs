use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::mpsc;

use crate::core::conf_sync::cache_server::{
    EventDispatch, ListData, ServerCache, WatchResponse,
};
use crate::core::utils::format_resource_info;
use crate::types::{
    EdgionGatewayConfig, EdgionTls, Gateway, GatewayClass, HTTPRoute, ResourceKind,
};
use anyhow::Result;
use crate::core::conf_sync::base_onf::GatewayClassBaseConf;

pub type GatewayClassKey = String;

// internal key
pub type NsNameKey = String;

pub enum ResourceItem {
    GatewayClass(GatewayClass),
    EdgionGatewayConfig(EdgionGatewayConfig),
    Gateway(Gateway),
    HTTPRoute(HTTPRoute),
    Service(Service),
    EndpointSlice(EndpointSlice),
    EdgionTls(EdgionTls),
    Secret(Secret),
}

// 1、单个controller只处理一种gateway_class
// 2、内部不做细分的全新配置，实际的权限配置全部由RBAC来控制他能取到哪些，取到哪些，就把哪些全部同步到对应的网关。（此处如果给予全部service/secret可见，那么对应的网关就可见）
// 3、只会处理对应route信息里的有些parentRefs是对应的，不然就不会处理
pub struct ConfigServer {
    gateway_class: Option<String>,
    pub base_conf: RwLock<GatewayClassBaseConf>,
    pub routes: RwLock<HashMap<GatewayClassKey, ServerCache<HTTPRoute>>>,
    pub services: RwLock<HashMap<GatewayClassKey, ServerCache<Service>>>,
    pub endpoint_slices: RwLock<HashMap<GatewayClassKey, ServerCache<EndpointSlice>>>,

    // this two should bond, otherwise, different gateway client will get all secrets.
    pub edgion_tls: RwLock<HashMap<GatewayClassKey, ServerCache<EdgionTls>>>,
    pub secrets: RwLock<HashMap<GatewayClassKey, ServerCache<Secret>>>,
}

pub struct ListDataSimple {
    pub data: String,
    pub resource_version: u64,
}

pub struct EventDataSimple {
    pub data: String,
    pub resource_version: u64,
    pub err: Option<String>,
}

impl ConfigServer {
    pub fn new(gateway_class: Option<String>) -> Self {
        Self {
            gateway_class,
            base_conf: RwLock::new(GatewayClassBaseConf::new()),
            routes: RwLock::new(HashMap::new()),
            services: RwLock::new(HashMap::new()),
            endpoint_slices: RwLock::new(HashMap::new()),
            edgion_tls: RwLock::new(HashMap::new()),
            secrets: RwLock::new(HashMap::new()),
        }
    }
    
    /// Get the configured gateway class name
    pub fn gateway_class(&self) -> Option<&String> {
        self.gateway_class.as_ref()
    }

    pub fn list(
        &self,
        key: &GatewayClassKey,
        kind: &ResourceKind,
    ) -> Result<ListDataSimple, String> {
        let (data_json, resource_version) = match kind {
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                return Err(format!("Base conf resources (GatewayClass, EdgionGatewayConfig, Gateway) are not available via list/watch API"));
            }
            ResourceKind::HTTPRoute => {
                let list_data = self
                    .list_routes(key)
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize HTTPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Service => {
                let list_data = self
                    .list_services(key)
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Service data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EndpointSlice => {
                let list_data = self
                    .list_endpoint_slices(key)
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EndpointSlice data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionTls => {
                let list_data = self
                    .list_edgion_tls(key)
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionTls data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Secret => {
                let list_data = self
                    .list_secrets(key)
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
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

    pub fn watch(
        &self,
        key: &GatewayClassKey,
        kind: &ResourceKind,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Result<mpsc::Receiver<EventDataSimple>, String> {
        let (tx, rx) = mpsc::channel(100);

        println!(
            "[ConfigCenter::watch] key={} kind={:?} client_id={} client_name={} from_version={}",
            key, kind, client_id, client_name, from_version
        );

        match kind {
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                return Err(format!("Base conf resources (GatewayClass, EdgionGatewayConfig, Gateway) are not available via list/watch API"));
            }
            ResourceKind::HTTPRoute => {
                let mut receiver = self
                    .watch_routes(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("HTTPRoute cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let WatchResponse {
                            events,
                            resource_version,
                            err,
                        } = response;

                        let events_json = match serde_json::to_string(&events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize HTTPRoute events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version,
                            err,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::Service => {
                let mut receiver = self
                    .watch_services(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("Service cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let WatchResponse {
                            events,
                            resource_version,
                            err,
                        } = response;

                        let events_json = match serde_json::to_string(&events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize Service events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version,
                            err,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::EndpointSlice => {
                let mut receiver = self
                    .watch_endpoint_slices(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("EndpointSlice cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let WatchResponse {
                            events,
                            resource_version,
                            err,
                        } = response;

                        let events_json = match serde_json::to_string(&events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize EndpointSlice events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version,
                            err,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::EdgionTls => {
                let mut receiver = self
                    .watch_edgion_tls(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("EdgionTls cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let WatchResponse {
                            events,
                            resource_version,
                            err,
                        } = response;

                        let events_json = match serde_json::to_string(&events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize EdgionTls events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version,
                            err,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::Secret => {
                let mut receiver = self
                    .watch_secrets(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("Secret cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let WatchResponse {
                            events,
                            resource_version,
                            err,
                        } = response;

                        let events_json = match serde_json::to_string(&events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize Secret events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version,
                            err,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
        }

        Ok(rx)
    }

    /// List gateway classes
    /// List all gateway class keys currently configured
    /// Returns the configured gateway class name if set
    pub fn list_all_gateway_class_keys(&self) -> Vec<String> {
        let keys: Vec<String> = if let Some(ref gc) = self.gateway_class {
            vec![gc.clone()]
        } else {
            Vec::new()
        };
        tracing::debug!(
            component = "config_server",
            event = "list_all_gateway_class_keys",
            count = keys.len(),
            keys = ?keys,
            "Listing all gateway class keys"
        );
        keys
    }

    /// List HTTP routes
    pub fn list_routes(&self, key: &str) -> Option<ListData<HTTPRoute>> {
        let routes = self.routes.read().unwrap();
        if let Some(cache) = routes.get(key) {
            Some(cache.list_owned())
        } else {
            None
        }
    }

    /// List services
    pub fn list_services(&self, key: &str) -> Option<ListData<Service>> {
        let services = self.services.read().unwrap();
        if let Some(cache) = services.get(key) {
            Some(cache.list_owned())
        } else {
            None
        }
    }

    /// List endpoint slices
    pub fn list_endpoint_slices(&self, key: &str) -> Option<ListData<EndpointSlice>> {
        let endpoint_slices = self.endpoint_slices.read().unwrap();
        if let Some(cache) = endpoint_slices.get(key) {
            Some(cache.list_owned())
        } else {
            None
        }
    }

    /// List Edgion TLS
    pub fn list_edgion_tls(&self, key: &str) -> Option<ListData<EdgionTls>> {
        let edgion_tls = self.edgion_tls.read().unwrap();
        if let Some(cache) = edgion_tls.get(key) {
            Some(cache.list_owned())
        } else {
            None
        }
    }

    /// List secrets
    pub fn list_secrets(&self, key: &str) -> Option<ListData<Secret>> {
        let secrets = self.secrets.read().unwrap();
        if let Some(cache) = secrets.get(key) {
            Some(cache.list_owned())
        } else {
            None
        }
    }


    /// Watch HTTP routes
    pub fn watch_routes(
        &self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<HTTPRoute>>> {
        let mut routes = self.routes.write().unwrap();
        let cache = routes.entry(key.to_string()).or_insert_with(|| {
            let mut cache = ServerCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch services
    pub fn watch_services(
        &self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<Service>>> {
        let mut services = self.services.write().unwrap();
        let cache = services.entry(key.to_string()).or_insert_with(|| {
            let mut cache = ServerCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch endpoint slices
    pub fn watch_endpoint_slices(
        &self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<EndpointSlice>>> {
        let mut endpoint_slices = self.endpoint_slices.write().unwrap();
        let cache = endpoint_slices.entry(key.to_string()).or_insert_with(|| {
            let mut cache = ServerCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch Edgion TLS
    pub fn watch_edgion_tls(
        &self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<EdgionTls>>> {
        let mut edgion_tls = self.edgion_tls.write().unwrap();
        let cache = edgion_tls.entry(key.to_string()).or_insert_with(|| {
            let mut cache = ServerCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch secrets
    pub fn watch_secrets(
        &self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<Secret>>> {
        let mut secrets = self.secrets.write().unwrap();
        let cache = secrets.entry(key.to_string()).or_insert_with(|| {
            let mut cache = ServerCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Print all configuration for a specific gateway class key
    pub async fn print_config(&self, key: &GatewayClassKey) {
        println!("=== ConfigCenter Config for GatewayClassKey: {} ===", key);

        // Base conf resources are stored in base_conf, not in separate caches
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
        if let Some(list_data) = self.list_routes(key) {
            println!(
                "HTTPRoutes (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, route) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(route));
            }
        } else {
            println!("HTTPRoutes: not found");
        }

        // Services
        if let Some(list_data) = self.list_services(key) {
            println!(
                "Services (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, svc) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(svc));
            }
        } else {
            println!("Services: not found");
        }

        // Endpoint Slices
        if let Some(list_data) = self.list_endpoint_slices(key) {
            println!(
                "EndpointSlices (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, es) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(es));
            }
        } else {
            println!("EndpointSlices: not found");
        }

        // Edgion TLS
        if let Some(list_data) = self.list_edgion_tls(key) {
            println!(
                "EdgionTls (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, tls) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(tls));
            }
        } else {
            println!("EdgionTls: not found");
        }

        // Secrets
        if let Some(list_data) = self.list_secrets(key) {
            println!(
                "Secrets (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, secret) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(secret));
            }
        } else {
            println!("Secrets: not found");
        }

        println!("=== End ConfigCenter Config ===\n");
    }
}

impl Default for ConfigServer {
    fn default() -> Self {
        Self::new(None)
    }
}
