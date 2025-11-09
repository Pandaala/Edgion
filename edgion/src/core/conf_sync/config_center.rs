use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::core::conf_sync::center_cache::{
    CenterCache, EventDispatch, ListData, Versionable, WatchResponse,
};
use crate::types::{
    EdgionGatewayConfig, EdgionTls, Gateway, GatewayClass, HTTPRoute, ResourceKind,
};
use anyhow::Result;

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

// todo, 所有的资源，都按照gatewayclass进行规制，对于多层映射的，后期可以通过添加filter的机制
pub struct ConfigCenter {
    pub gateway_classes: HashMap<GatewayClassKey, CenterCache<GatewayClass>>,
    pub edgion_gateway_configs: HashMap<GatewayClassKey, CenterCache<EdgionGatewayConfig>>,
    pub gateways: HashMap<GatewayClassKey, CenterCache<Gateway>>,
    pub routes: HashMap<GatewayClassKey, CenterCache<HTTPRoute>>,
    pub services: HashMap<GatewayClassKey, CenterCache<Service>>,
    pub endpoint_slices: HashMap<GatewayClassKey, CenterCache<EndpointSlice>>,
    pub edgion_tls: HashMap<GatewayClassKey, CenterCache<EdgionTls>>,
    pub secrets: HashMap<GatewayClassKey, CenterCache<Secret>>,
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

impl ConfigCenter {
    pub fn new() -> Self {
        Self {
            gateway_classes: HashMap::new(),
            edgion_gateway_configs: HashMap::new(),
            gateways: HashMap::new(),
            routes: HashMap::new(),
            services: HashMap::new(),
            endpoint_slices: HashMap::new(),
            edgion_tls: HashMap::new(),
            secrets: HashMap::new(),
        }
    }

    pub async fn list(
        &self,
        key: &GatewayClassKey,
        kind: &ResourceKind,
    ) -> Result<ListDataSimple, String> {
        let (data_json, resource_version) = match kind {
            ResourceKind::GatewayClass => {
                let list_data = self
                    .list_gateway_classes(key)
                    .await
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize GatewayClass data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionGatewayConfig => {
                let list_data = self
                    .list_edgion_gateway_configs(key)
                    .await
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionGatewayConfig data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Gateway => {
                let list_data = self
                    .list_gateways(key)
                    .await
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Gateway data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::HTTPRoute => {
                let list_data = self
                    .list_routes(key)
                    .await
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize HTTPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Service => {
                let list_data = self
                    .list_services(key)
                    .await
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Service data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EndpointSlice => {
                let list_data = self
                    .list_endpoint_slices(key)
                    .await
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EndpointSlice data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionTls => {
                let list_data = self
                    .list_edgion_tls(key)
                    .await
                    .unwrap_or_else(|| ListData::new(Vec::new(), 0));
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionTls data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Secret => {
                let list_data = self
                    .list_secrets(key)
                    .await
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
        &mut self,
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
            ResourceKind::GatewayClass => {
                let client_id_log = client_id.clone();
                let client_name_log = client_name.clone();

                let mut receiver = match self.watch_gateway_classes(
                    key,
                    client_id,
                    client_name,
                    from_version,
                ) {
                    Some(receiver) => {
                        println!(
                            "[ConfigCenter::watch] GatewayClass cache hit key={} client_id={} client_name={}",
                            key,
                            client_id_log,
                            client_name_log
                        );
                        receiver
                    }
                    None => {
                        let available_keys: Vec<_> = self.gateway_classes.keys().cloned().collect();
                        println!(
                            "[ConfigCenter::watch] GatewayClass cache miss key={} client_id={} client_name={} available_keys={:?}",
                            key,
                            client_id_log,
                            client_name_log,
                            available_keys
                        );
                        return Err(format!("GatewayClass cache not found for key: {}", key));
                    }
                };

                println!(
                    "[ConfigCenter::watch] GatewayClass watch established key={} client_id={} client_name={} from_version={}",
                    key,
                    client_id_log,
                    client_name_log,
                    from_version
                );
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
                                eprintln!("Failed to serialize GatewayClass events: {}", e);
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
            ResourceKind::EdgionGatewayConfig => {
                let mut receiver = self
                    .watch_edgion_gateway_configs(key, client_id, client_name, from_version)
                    .ok_or_else(|| {
                        format!("EdgionGatewayConfig cache not found for key: {}", key)
                    })?;
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
                                eprintln!("Failed to serialize EdgionGatewayConfig events: {}", e);
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
            ResourceKind::Gateway => {
                let mut receiver = self
                    .watch_gateways(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("Gateway cache not found for key: {}", key))?;
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
                                eprintln!("Failed to serialize Gateway events: {}", e);
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
    pub async fn list_gateway_classes(&self, key: &str) -> Option<ListData<GatewayClass>> {
        if let Some(cache) = self.gateway_classes.get(key) {
            Some(cache.list_owned().await)
        } else {
            None
        }
    }

    pub async fn list_edgion_gateway_configs(
        &self,
        key: &str,
    ) -> Option<ListData<EdgionGatewayConfig>> {
        if let Some(cache) = self.edgion_gateway_configs.get(key) {
            Some(cache.list_owned().await)
        } else {
            None
        }
    }

    /// List gateways
    pub async fn list_gateways(&self, key: &str) -> Option<ListData<Gateway>> {
        if let Some(cache) = self.gateways.get(key) {
            Some(cache.list_owned().await)
        } else {
            None
        }
    }

    /// List HTTP routes
    pub async fn list_routes(&self, key: &str) -> Option<ListData<HTTPRoute>> {
        if let Some(cache) = self.routes.get(key) {
            Some(cache.list_owned().await)
        } else {
            None
        }
    }

    /// List services
    pub async fn list_services(&self, key: &str) -> Option<ListData<Service>> {
        if let Some(cache) = self.services.get(key) {
            Some(cache.list_owned().await)
        } else {
            None
        }
    }

    /// List endpoint slices
    pub async fn list_endpoint_slices(&self, key: &str) -> Option<ListData<EndpointSlice>> {
        if let Some(cache) = self.endpoint_slices.get(key) {
            Some(cache.list_owned().await)
        } else {
            None
        }
    }

    /// List Edgion TLS
    pub async fn list_edgion_tls(&self, key: &str) -> Option<ListData<EdgionTls>> {
        if let Some(cache) = self.edgion_tls.get(key) {
            Some(cache.list_owned().await)
        } else {
            None
        }
    }

    /// List secrets
    pub async fn list_secrets(&self, key: &str) -> Option<ListData<Secret>> {
        if let Some(cache) = self.secrets.get(key) {
            Some(cache.list_owned().await)
        } else {
            None
        }
    }

    /// Watch gateway classes
    pub fn watch_gateway_classes(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<GatewayClass>>> {
        let cache = self
            .gateway_classes
            .entry(key.to_string())
            .or_insert_with(|| {
                let mut cache = CenterCache::new(1000);
                EventDispatch::set_ready(&mut cache);
                cache
            });
        Some(cache.watch(client_id, client_name, from_version))
    }

    pub fn watch_edgion_gateway_configs(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<EdgionGatewayConfig>>> {
        let cache = self
            .edgion_gateway_configs
            .entry(key.to_string())
            .or_insert_with(|| {
                let mut cache = CenterCache::new(1000);
                EventDispatch::set_ready(&mut cache);
                cache
            });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch gateways
    pub fn watch_gateways(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<Gateway>>> {
        let cache = self.gateways.entry(key.to_string()).or_insert_with(|| {
            let mut cache = CenterCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch HTTP routes
    pub fn watch_routes(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<HTTPRoute>>> {
        let cache = self.routes.entry(key.to_string()).or_insert_with(|| {
            let mut cache = CenterCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch services
    pub fn watch_services(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<Service>>> {
        let cache = self.services.entry(key.to_string()).or_insert_with(|| {
            let mut cache = CenterCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch endpoint slices
    pub fn watch_endpoint_slices(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<EndpointSlice>>> {
        let cache = self
            .endpoint_slices
            .entry(key.to_string())
            .or_insert_with(|| {
                let mut cache = CenterCache::new(1000);
                EventDispatch::set_ready(&mut cache);
                cache
            });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch Edgion TLS
    pub fn watch_edgion_tls(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<EdgionTls>>> {
        let cache = self.edgion_tls.entry(key.to_string()).or_insert_with(|| {
            let mut cache = CenterCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Watch secrets
    pub fn watch_secrets(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<Secret>>> {
        let cache = self.secrets.entry(key.to_string()).or_insert_with(|| {
            let mut cache = CenterCache::new(1000);
            EventDispatch::set_ready(&mut cache);
            cache
        });
        Some(cache.watch(client_id, client_name, from_version))
    }

    /// Print all configuration for a specific gateway class key
    pub async fn print_config(&self, key: &GatewayClassKey) {
        println!("=== ConfigCenter Config for GatewayClassKey: {} ===", key);

        // Gateway Classes
        if let Some(list_data) = self.list_gateway_classes(key).await {
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
        } else {
            println!("GatewayClasses: not found");
        }

        if let Some(list_data) = self.list_edgion_gateway_configs(key).await {
            println!(
                "EdgionGatewayConfigs (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, egwc) in list_data.data.iter().enumerate() {
                println!(
                    "  [{}] {}",
                    idx,
                    serde_json::to_string(egwc)
                        .unwrap_or_else(|_| "serialization error".to_string())
                );
            }
        } else {
            println!("EdgionGatewayConfigs: not found");
        }

        // Gateways
        if let Some(list_data) = self.list_gateways(key).await {
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
        } else {
            println!("Gateways: not found");
        }

        // HTTP Routes
        if let Some(list_data) = self.list_routes(key).await {
            println!(
                "HTTPRoutes (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, route) in list_data.data.iter().enumerate() {
                println!(
                    "  [{}] {}",
                    idx,
                    serde_json::to_string(route)
                        .unwrap_or_else(|_| "serialization error".to_string())
                );
            }
        } else {
            println!("HTTPRoutes: not found");
        }

        // Services
        if let Some(list_data) = self.list_services(key).await {
            println!(
                "Services (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, svc) in list_data.data.iter().enumerate() {
                println!(
                    "  [{}] {}",
                    idx,
                    serde_json::to_string(svc)
                        .unwrap_or_else(|_| "serialization error".to_string())
                );
            }
        } else {
            println!("Services: not found");
        }

        // Endpoint Slices
        if let Some(list_data) = self.list_endpoint_slices(key).await {
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
        } else {
            println!("EndpointSlices: not found");
        }

        // Edgion TLS
        if let Some(list_data) = self.list_edgion_tls(key).await {
            println!(
                "EdgionTls (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, tls) in list_data.data.iter().enumerate() {
                println!(
                    "  [{}] {}",
                    idx,
                    serde_json::to_string(tls)
                        .unwrap_or_else(|_| "serialization error".to_string())
                );
            }
        } else {
            println!("EdgionTls: not found");
        }

        // Secrets
        if let Some(list_data) = self.list_secrets(key).await {
            println!(
                "Secrets (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, secret) in list_data.data.iter().enumerate() {
                println!(
                    "  [{}] {}",
                    idx,
                    serde_json::to_string(secret)
                        .unwrap_or_else(|_| "serialization error".to_string())
                );
            }
        } else {
            println!("Secrets: not found");
        }

        println!("=== End ConfigCenter Config ===\n");
    }
}

impl Default for ConfigCenter {
    fn default() -> Self {
        Self::new()
    }
}
