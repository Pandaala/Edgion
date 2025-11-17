use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::sync::RwLock;
use tokio::sync::mpsc;

use crate::core::conf_sync::cache_server::{
    ListData, ServerCache, WatchResponse,
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
    pub routes: ServerCache<HTTPRoute>,
    pub services: ServerCache<Service>,
    pub endpoint_slices: ServerCache<EndpointSlice>,
    pub edgion_tls: ServerCache<EdgionTls>,
    pub secrets: ServerCache<Secret>,
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

pub struct BaseConfData {
    pub gateway_class: String,
    pub edgion_gateway_config: String,
    pub gateways: String,
}

impl ConfigServer {
    pub fn new(gateway_class: Option<String>) -> Self {
        Self {
            gateway_class,
            base_conf: RwLock::new(GatewayClassBaseConf::new()),
            routes: ServerCache::new(200),
            services: ServerCache::new(200),
            endpoint_slices: ServerCache::new(200),
            edgion_tls: ServerCache::new(200),
            secrets: ServerCache::new(200),
        }
    }
    
    /// Get the configured gateway class name
    pub fn gateway_class(&self) -> Option<&String> {
        self.gateway_class.as_ref()
    }

    /// Get base configuration for a specific gateway class
    /// Returns the base conf data as JSON strings
    pub fn get_base_conf(&self, gateway_class: &str) -> Result<BaseConfData, String> {
        // Verify gateway class matches if configured
        if let Some(ref configured_gc) = self.gateway_class {
            if configured_gc != gateway_class {
                return Err(format!(
                    "Gateway class mismatch: expected {}, got {}",
                    configured_gc, gateway_class
                ));
            }
        }

        let base_conf = self.base_conf.read().unwrap();
        
        let gateway_class_json = base_conf.gateway_class()
            .and_then(|gc| serde_json::to_string(gc).ok())
            .unwrap_or_default();
        
        let edgion_gateway_config_json = base_conf.edgion_gateway_config()
            .and_then(|egwc| serde_json::to_string(egwc).ok())
            .unwrap_or_default();
        
        let gateways_json = serde_json::to_string(base_conf.gateways())
            .map_err(|e| format!("Failed to serialize gateways: {}", e))?;

        Ok(BaseConfData {
            gateway_class: gateway_class_json,
            edgion_gateway_config: edgion_gateway_config_json,
            gateways: gateways_json,
        })
    }

    pub fn list(
        &self,
        _key: &GatewayClassKey,
        kind: &ResourceKind,
    ) -> Result<ListDataSimple, String> {
        let (data_json, resource_version) = match kind {
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                return Err(format!("Base conf resources (GatewayClass, EdgionGatewayConfig, Gateway) are not available via list/watch API"));
            }
            ResourceKind::HTTPRoute => {
                let list_data = self.list_routes();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize HTTPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Service => {
                let list_data = self.list_services();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Service data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EndpointSlice => {
                let list_data = self.list_endpoint_slices();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EndpointSlice data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionTls => {
                let list_data = self.list_edgion_tls();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionTls data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Secret => {
                let list_data = self.list_secrets();
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
        _key: &GatewayClassKey,
        kind: &ResourceKind,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Result<mpsc::Receiver<EventDataSimple>, String> {
        let (tx, rx) = mpsc::channel(100);

        println!(
            "[ConfigCenter::watch] kind={:?} client_id={} client_name={} from_version={}",
            kind, client_id, client_name, from_version
        );

        match kind {
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                return Err(format!("Base conf resources (GatewayClass, EdgionGatewayConfig, Gateway) are not available via list/watch API"));
            }
            ResourceKind::HTTPRoute => {
                let mut receiver = self
                    .watch_routes(client_id, client_name, from_version);
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
                    .watch_services(client_id, client_name, from_version);
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
                    .watch_endpoint_slices(client_id, client_name, from_version);
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
                    .watch_edgion_tls(client_id, client_name, from_version);
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
                    .watch_secrets(client_id, client_name, from_version);
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


    /// Watch HTTP routes
    pub fn watch_routes(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<HTTPRoute>> {
        self.routes.watch(client_id, client_name, from_version)
    }

    /// Watch services
    pub fn watch_services(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<Service>> {
        self.services.watch(client_id, client_name, from_version)
    }

    /// Watch endpoint slices
    pub fn watch_endpoint_slices(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<EndpointSlice>> {
        self.endpoint_slices.watch(client_id, client_name, from_version)
    }

    /// Watch Edgion TLS
    pub fn watch_edgion_tls(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<EdgionTls>> {
        self.edgion_tls.watch(client_id, client_name, from_version)
    }

    /// Watch secrets
    pub fn watch_secrets(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<Secret>> {
        self.secrets.watch(client_id, client_name, from_version)
    }

    /// Print all configuration for a specific gateway class key
    pub async fn print_config(&self) {
        println!("\n==========================");

        // Base conf resources are stored in base_conf
        let base_conf = self.base_conf.read().unwrap();
        if let Some(gc) = base_conf.gateway_class() {
            println!("GatewayClass:");
            println!("  [0] {}", format_resource_info(gc));
        }

        if let Some(egwc) = base_conf.edgion_gateway_config() {
            println!("EdgionGatewayConfig:");
            println!("  [0] {}", format_resource_info(egwc));
        }

        let gateways = base_conf.gateways();
        if !gateways.is_empty() {
            println!("Gateways (count: {}):", gateways.len());
            for (idx, gw) in gateways.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(gw));
            }
        }
        drop(base_conf);

        // HTTP Routes
        let list_data = self.list_routes();
        if !list_data.data.is_empty() {
            println!(
                "HTTPRoutes (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, route) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(route));
            }
        }

        // Services
        let list_data = self.list_services();
        if !list_data.data.is_empty() {
            println!(
                "Services (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, svc) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(svc));
            }
        }

        // Endpoint Slices
        let list_data = self.list_endpoint_slices();
        if !list_data.data.is_empty() {
            println!(
                "EndpointSlices (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, es) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(es));
            }
        }

        // Edgion TLS
        let list_data = self.list_edgion_tls();
        if !list_data.data.is_empty() {
            println!(
                "EdgionTls (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, tls) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(tls));
            }
        }

        // Secrets
        let list_data = self.list_secrets();
        if !list_data.data.is_empty() {
            println!(
                "Secrets (count: {}, version: {}):",
                list_data.data.len(),
                list_data.resource_version
            );
            for (idx, secret) in list_data.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(secret));
            }
        }
    }
}

impl Default for ConfigServer {
    fn default() -> Self {
        Self::new(None)
    }
}
