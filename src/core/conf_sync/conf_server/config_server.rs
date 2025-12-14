use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::sync::RwLock;
use tokio::sync::mpsc;

use crate::types::GatewayBaseConf;
use crate::core::conf_sync::cache_server::ServerCache;
use crate::core::conf_sync::types::{ListData, WatchResponse};
use crate::core::utils::format_resource_info;
use crate::types::prelude_resources::*;
use anyhow::Result;

// internal key
pub type NsNameKey = String;

pub enum ResourceItem {
    GatewayClass(GatewayClass),
    EdgionGatewayConfig(EdgionGatewayConfig),
    Gateway(Gateway),
    HTTPRoute(HTTPRoute),
    GRPCRoute(GRPCRoute),
    TCPRoute(TCPRoute),
    UDPRoute(UDPRoute),
    TLSRoute(TLSRoute),
    Service(Service),
    EndpointSlice(EndpointSlice),
    EdgionTls(EdgionTls),
    EdgionPlugins(EdgionPlugins),
    PluginMetaData(PluginMetaData),
    Secret(Secret),
}

// 1、单个controller只处理一种gateway_class
// 2、内部不做细分的全新配置，实际的权限配置全部由RBAC来控制他能取到哪些，取到哪些，就把哪些全部同步到对应的网关。（此处如果给予全部service/secret可见，那么对应的网关就可见）
// 3、只会处理对应route信息里的有些parentRefs是对应的，不然就不会处理
pub struct ConfigServer {
    pub base_conf: RwLock<GatewayBaseConf>,
    pub routes: ServerCache<HTTPRoute>,
    pub grpc_routes: ServerCache<GRPCRoute>,
    pub tcp_routes: ServerCache<TCPRoute>,
    pub udp_routes: ServerCache<UDPRoute>,
    pub tls_routes: ServerCache<TLSRoute>,
    pub services: ServerCache<Service>,
    pub endpoint_slices: ServerCache<EndpointSlice>,
    pub edgion_tls: ServerCache<EdgionTls>,
    pub edgion_plugins: ServerCache<EdgionPlugins>,
    pub plugin_metadata: ServerCache<PluginMetaData>,
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
    pub base_conf: String,
}

impl ConfigServer {
    pub fn new(base_conf: GatewayBaseConf) -> Self {
        Self {
            base_conf: RwLock::new(base_conf),
            routes: ServerCache::new(200),
            grpc_routes: ServerCache::new(200),
            tcp_routes: ServerCache::new(200),
            udp_routes: ServerCache::new(200),
            tls_routes: ServerCache::new(200),
            services: ServerCache::new(200),
            endpoint_slices: ServerCache::new(200),
            edgion_tls: ServerCache::new(200),
            edgion_plugins: ServerCache::new(200),
            plugin_metadata: ServerCache::new(200),
            secrets: ServerCache::new(200),
        }
    }

    /// Get the configured gateway class name from base_conf
    pub fn gateway_class(&self) -> Option<String> {
        let base_conf_guard = self.base_conf.read().unwrap();
        base_conf_guard.gateway_class_name().map(|s| s.clone())
    }

    /// Get base configuration for a specific gateway class
    /// Returns the base conf data as JSON string
    pub fn get_base_conf(&self, gateway_class: &str) -> Result<BaseConfData, String> {
        let base_conf_guard = self.base_conf.read().unwrap();
        
        // Verify gateway class matches if configured
        if let Some(configured_gc) = base_conf_guard.gateway_class_name() {
            if configured_gc != gateway_class {
                return Err(format!(
                    "Gateway class mismatch: expected {}, got {}",
                    configured_gc, gateway_class
                ));
            }
        }

        let base_conf_json = serde_json::to_string(&*base_conf_guard)
            .map_err(|e| format!("Failed to serialize base conf: {}", e))?;

        Ok(BaseConfData {
            base_conf: base_conf_json,
        })
    }

    pub fn list(&self, kind: &ResourceKind) -> Result<ListDataSimple, String> {
        let (data_json, resource_version) = match kind {
            ResourceKind::Unspecified => {
                return Err("Resource kind unspecified".to_string());
            }
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                return Err("Base conf resources (GatewayClass, EdgionGatewayConfig, Gateway) are not available via list/watch API".to_string());
            }
            ResourceKind::HTTPRoute => {
                let list_data = self.list_routes();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize HTTPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::GRPCRoute => {
                let list_data = self.list_grpc_routes();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize GRPCRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::TCPRoute => {
                let list_data = self.list_tcp_routes();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize TCPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::UDPRoute => {
                let list_data = self.list_udp_routes();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize UDPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::TLSRoute => {
                let list_data = self.list_tls_routes();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize TLSRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::PluginMetaData => {
                let list_data = self.list_plugin_metadata();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize PluginMetaData data: {}", e))?;
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
            ResourceKind::EdgionPlugins => {
                let list_data = self.list_edgion_plugins();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionPlugins data: {}", e))?;
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
            ResourceKind::Unspecified => {
                return Err("Resource kind unspecified".to_string());
            }
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                return Err("Base conf resources (GatewayClass, EdgionGatewayConfig, Gateway) are not available via list/watch API".to_string());
            }
            ResourceKind::GRPCRoute => {
                let mut receiver = self.watch_grpc_routes(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize GRPCRoute events: {}", e);
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
            ResourceKind::TCPRoute => {
                let mut receiver = self.watch_tcp_routes(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize TCPRoute events: {}", e);
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
            ResourceKind::UDPRoute => {
                let mut receiver = self.watch_udp_routes(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize UDPRoute events: {}", e);
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
            ResourceKind::TLSRoute => {
                let mut receiver = self.watch_tls_routes(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize TLSRoute events: {}", e);
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
            ResourceKind::PluginMetaData => {
                let mut receiver = self.watch_plugin_metadata(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize PluginMetaData events: {}", e);
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
                let mut receiver = self.watch_routes(client_id, client_name, from_version);
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
                let mut receiver = self.watch_services(client_id, client_name, from_version);
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
                let mut receiver = self.watch_endpoint_slices(client_id, client_name, from_version);
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
                let mut receiver = self.watch_edgion_tls(client_id, client_name, from_version);
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
            ResourceKind::EdgionPlugins => {
                let mut receiver = self.watch_edgion_plugins(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize EdgionPlugins events: {}", e);
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
                let mut receiver = self.watch_secrets(client_id, client_name, from_version);
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

    pub fn list_grpc_routes(&self) -> ListData<GRPCRoute> {
        self.grpc_routes.list_owned()
    }

    pub fn list_tcp_routes(&self) -> ListData<TCPRoute> {
        self.tcp_routes.list_owned()
    }

    pub fn list_udp_routes(&self) -> ListData<UDPRoute> {
        self.udp_routes.list_owned()
    }

    pub fn list_tls_routes(&self) -> ListData<TLSRoute> {
        self.tls_routes.list_owned()
    }

    pub fn list_plugin_metadata(&self) -> ListData<PluginMetaData> {
        self.plugin_metadata.list_owned()
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

    /// List Edgion Plugins
    pub fn list_edgion_plugins(&self) -> ListData<EdgionPlugins> {
        self.edgion_plugins.list_owned()
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

    /// Watch gRPC routes
    pub fn watch_grpc_routes(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<GRPCRoute>> {
        self.grpc_routes.watch(client_id, client_name, from_version)
    }

    /// Watch TCP routes
    pub fn watch_tcp_routes(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<TCPRoute>> {
        self.tcp_routes.watch(client_id, client_name, from_version)
    }

    /// Watch UDP routes
    pub fn watch_udp_routes(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<UDPRoute>> {
        self.udp_routes.watch(client_id, client_name, from_version)
    }

    /// Watch TLS routes
    pub fn watch_tls_routes(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<TLSRoute>> {
        self.tls_routes.watch(client_id, client_name, from_version)
    }

    /// Watch plugin metadata
    pub fn watch_plugin_metadata(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<PluginMetaData>> {
        self.plugin_metadata.watch(client_id, client_name, from_version)
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

    /// Watch Edgion Plugins
    pub fn watch_edgion_plugins(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<EdgionPlugins>> {
        self.edgion_plugins.watch(client_id, client_name, from_version)
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
        let base_conf_guard = self.base_conf.read().unwrap();
            println!("GatewayClass:");
        println!("  [0] {}", format_resource_info(base_conf_guard.gateway_class()));

            println!("EdgionGatewayConfig:");
        println!("  [0] {}", format_resource_info(base_conf_guard.edgion_gateway_config()));

        let gateways = base_conf_guard.gateways();
            if !gateways.is_empty() {
                println!("Gateways (count: {}):", gateways.len());
                for (idx, gw) in gateways.iter().enumerate() {
                    println!("  [{}] {}", idx, format_resource_info(gw));
                }
        }
        drop(base_conf_guard);

        println!(""); // Empty line before user conf resources
        
        // HTTP Routes
        tracing::debug!(component = "config_server", event = "listing_routes", "About to call list_routes");
        let list_data = self.list_routes();
        tracing::debug!(component = "config_server", event = "listed_routes", count = list_data.data.len(), "list_routes returned");
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

        // Edgion Plugins
        let list_data = self.list_edgion_plugins();
        println!(
            "EdgionPlugins (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, plugin) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(plugin));
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
        
        println!("==========================\n");
    }
}
