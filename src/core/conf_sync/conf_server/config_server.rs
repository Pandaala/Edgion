use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::secret_ref::SecretRefManager;
use crate::core::conf_sync::cache_server::ServerCache;
use crate::core::conf_sync::traits::CacheEventDispatch;
use crate::core::conf_sync::types::{ListData, WatchResponse};
use crate::core::utils::format_resource_info;
use crate::types::prelude_resources::*;
use anyhow::Result;

// internal key
pub type NsNameKey = String;

#[allow(clippy::large_enum_variant)]
pub enum ResourceItem {
    GatewayClass(GatewayClass),
    EdgionGatewayConfig(EdgionGatewayConfig),
    Gateway(Gateway),
    HTTPRoute(HTTPRoute),
    GRPCRoute(GRPCRoute),
    TCPRoute(TCPRoute),
    UDPRoute(UDPRoute),
    TLSRoute(TLSRoute),
    LinkSys(LinkSys),
    Service(Service),
    EndpointSlice(EndpointSlice),
    Endpoint(Endpoints),
    EdgionTls(EdgionTls),
    EdgionPlugins(EdgionPlugins),
    EdgionStreamPlugins(EdgionStreamPlugins),
    ReferenceGrant(ReferenceGrant),
    BackendTLSPolicy(BackendTLSPolicy),
    PluginMetaData(PluginMetaData),
    Secret(Secret),
}

// 1. Each controller handles only one gateway_class
// 2. No internal subdivision; actual permissions are controlled by RBAC, which determines what can be accessed and synced to the gateway. (If all services/secrets are visible, then they are visible to the gateway)
// 3. Only processes routes where parentRefs match; otherwise, they are ignored
pub struct ConfigServer {
    // Base conf resources now use dedicated ServerCache (same as other resources)
    pub gateway_classes: ServerCache<GatewayClass>,
    pub gateways: ServerCache<Gateway>,
    pub edgion_gateway_configs: ServerCache<EdgionGatewayConfig>,
    pub routes: ServerCache<HTTPRoute>,
    pub grpc_routes: ServerCache<GRPCRoute>,
    pub tcp_routes: ServerCache<TCPRoute>,
    pub udp_routes: ServerCache<UDPRoute>,
    pub tls_routes: ServerCache<TLSRoute>,
    pub link_sys: ServerCache<LinkSys>,
    pub services: ServerCache<Service>,
    pub endpoint_slices: ServerCache<EndpointSlice>,
    pub endpoints: ServerCache<Endpoints>,
    pub edgion_tls: ServerCache<EdgionTls>,
    pub edgion_plugins: ServerCache<EdgionPlugins>,
    pub edgion_stream_plugins: ServerCache<EdgionStreamPlugins>,
    pub reference_grants: ServerCache<ReferenceGrant>,
    pub backend_tls_policies: ServerCache<BackendTLSPolicy>,
    pub plugin_metadata: ServerCache<PluginMetaData>,
    pub secrets: ServerCache<Secret>,
    pub secret_ref_manager: Arc<SecretRefManager>,
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
    pub fn new(conf_sync_config: &crate::core::cli::config::ConfSyncConfig) -> Self {
        Self {
            gateway_classes: ServerCache::new(conf_sync_config.gateway_classes_capacity),
            gateways: ServerCache::new(conf_sync_config.gateways_capacity),
            edgion_gateway_configs: ServerCache::new(conf_sync_config.edgion_gateway_configs_capacity),
            routes: ServerCache::new(conf_sync_config.routes_capacity),
            grpc_routes: ServerCache::new(conf_sync_config.grpc_routes_capacity),
            tcp_routes: ServerCache::new(conf_sync_config.tcp_routes_capacity),
            udp_routes: ServerCache::new(conf_sync_config.udp_routes_capacity),
            tls_routes: ServerCache::new(conf_sync_config.tls_routes_capacity),
            link_sys: ServerCache::new(conf_sync_config.link_sys_capacity),
            services: ServerCache::new(conf_sync_config.services_capacity),
            endpoint_slices: ServerCache::new(conf_sync_config.endpoint_slices_capacity),
            endpoints: ServerCache::new(conf_sync_config.endpoints_capacity),
            edgion_tls: ServerCache::new(conf_sync_config.edgion_tls_capacity),
            edgion_plugins: ServerCache::new(conf_sync_config.edgion_plugins_capacity),
            edgion_stream_plugins: ServerCache::new(conf_sync_config.edgion_stream_plugins_capacity),
            reference_grants: ServerCache::new(conf_sync_config.reference_grants_capacity),
            backend_tls_policies: ServerCache::new(conf_sync_config.backend_tls_policies_capacity),
            plugin_metadata: ServerCache::new(conf_sync_config.plugin_metadata_capacity),
            secrets: ServerCache::new(conf_sync_config.secrets_capacity),
            secret_ref_manager: Arc::new(SecretRefManager::new()),
        }
    }

    /// Get SecretRefManager statistics for monitoring
    pub fn secret_ref_stats(&self) -> super::secret_ref::RefManagerStats {
        self.secret_ref_manager.stats()
    }

    /// Print SecretRefManager statistics
    pub fn print_secret_ref_stats(&self) {
        let stats = self.secret_ref_manager.stats();
        tracing::info!(
            component = "config_server",
            event = "secret_ref_stats",
            secret_count = stats.secret_count,
            resource_count = stats.resource_count,
            total_references = stats.total_references,
            "Secret reference manager statistics"
        );
    }

    pub fn list(&self, kind: &ResourceKind) -> Result<ListDataSimple, String> {
        let (data_json, resource_version) = match kind {
            ResourceKind::Unspecified => return Err("Resource kind unspecified".to_string()),
            ResourceKind::GatewayClass => self.list_gateway_classes().to_json("GatewayClass")?,
            ResourceKind::EdgionGatewayConfig => self.list_edgion_gateway_configs().to_json("EdgionGatewayConfig")?,
            ResourceKind::Gateway => self.list_gateways().to_json("Gateway")?,
            ResourceKind::HTTPRoute => self.list_routes().to_json("HTTPRoute")?,
            ResourceKind::GRPCRoute => self.list_grpc_routes().to_json("GRPCRoute")?,
            ResourceKind::TCPRoute => self.list_tcp_routes().to_json("TCPRoute")?,
            ResourceKind::UDPRoute => self.list_udp_routes().to_json("UDPRoute")?,
            ResourceKind::TLSRoute => self.list_tls_routes().to_json("TLSRoute")?,
            ResourceKind::LinkSys => self.list_link_sys().to_json("LinkSys")?,
            ResourceKind::PluginMetaData => self.list_plugin_metadata().to_json("PluginMetaData")?,
            ResourceKind::Service => self.list_services().to_json("Service")?,
            ResourceKind::EndpointSlice => self.list_endpoint_slices().to_json("EndpointSlice")?,
            ResourceKind::Endpoint => self.list_endpoints().to_json("Endpoints")?,
            ResourceKind::EdgionTls => self.list_edgion_tls().to_json("EdgionTls")?,
            ResourceKind::EdgionPlugins => self.list_edgion_plugins().to_json("EdgionPlugins")?,
            ResourceKind::EdgionStreamPlugins => self.list_edgion_stream_plugins().to_json("EdgionStreamPlugins")?,
            ResourceKind::ReferenceGrant => self.list_reference_grants().to_json("ReferenceGrant")?,
            ResourceKind::BackendTLSPolicy => self.list_backend_tls_policies().to_json("BackendTLSPolicy")?,
            ResourceKind::Secret => self.list_secrets().to_json("Secret")?,
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
            ResourceKind::GatewayClass => {
                let mut receiver = self.watch_gateway_classes(client_id, client_name, from_version);
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
                let mut receiver = self.watch_edgion_gateway_configs(client_id, client_name, from_version);
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
                let mut receiver = self.watch_gateways(client_id, client_name, from_version);
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
            ResourceKind::LinkSys => {
                let mut receiver = self.watch_link_sys(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize LinkSys events: {}", e);
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
            ResourceKind::Endpoint => {
                let mut receiver = self.watch_endpoints(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize Endpoints events: {}", e);
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
            ResourceKind::EdgionStreamPlugins => {
                let mut receiver = self.watch_edgion_stream_plugins(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize EdgionStreamPlugins events: {}", e);
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
            ResourceKind::ReferenceGrant => {
                let mut receiver = self.watch_reference_grants(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize ReferenceGrant events: {}", e);
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
            ResourceKind::BackendTLSPolicy => {
                let mut receiver = self.watch_backend_tls_policies(client_id, client_name, from_version);
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
                                eprintln!("Failed to serialize BackendTLSPolicy events: {}", e);
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

    pub fn list_link_sys(&self) -> ListData<LinkSys> {
        self.link_sys.list_owned()
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

    /// List Endpoints
    pub fn list_endpoints(&self) -> ListData<Endpoints> {
        self.endpoints.list_owned()
    }

    /// List Edgion TLS
    pub fn list_edgion_tls(&self) -> ListData<EdgionTls> {
        self.edgion_tls.list_owned()
    }

    /// List Edgion Plugins
    pub fn list_edgion_plugins(&self) -> ListData<EdgionPlugins> {
        self.edgion_plugins.list_owned()
    }

    /// List EdgionStreamPlugins
    pub fn list_edgion_stream_plugins(&self) -> ListData<EdgionStreamPlugins> {
        self.edgion_stream_plugins.list_owned()
    }

    /// List ReferenceGrants
    pub fn list_reference_grants(&self) -> ListData<ReferenceGrant> {
        self.reference_grants.list_owned()
    }

    pub fn list_backend_tls_policies(&self) -> ListData<BackendTLSPolicy> {
        self.backend_tls_policies.list_owned()
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

    /// Watch LinkSys
    pub fn watch_link_sys(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<LinkSys>> {
        self.link_sys.watch(client_id, client_name, from_version)
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

    /// Watch Endpoints
    pub fn watch_endpoints(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<Endpoints>> {
        self.endpoints.watch(client_id, client_name, from_version)
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

    /// Watch EdgionStreamPlugins
    pub fn watch_edgion_stream_plugins(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<EdgionStreamPlugins>> {
        self.edgion_stream_plugins.watch(client_id, client_name, from_version)
    }

    /// Watch ReferenceGrants
    pub fn watch_reference_grants(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<ReferenceGrant>> {
        self.reference_grants.watch(client_id, client_name, from_version)
    }

    pub fn watch_backend_tls_policies(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<BackendTLSPolicy>> {
        self.backend_tls_policies.watch(client_id, client_name, from_version)
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

        // Base conf resources are now stored in ServerCache (like other resources)
        let list_data = self.list_gateway_classes();
        println!(
            "GatewayClass (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, gc) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(gc));
        }

        let list_data = self.list_edgion_gateway_configs();
        println!(
            "EdgionGatewayConfig (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, egwc) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(egwc));
        }

        let list_data = self.list_gateways();
        println!(
            "Gateway (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, gw) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(gw));
        }

        println!(); // Empty line before user conf resources

        // HTTP Routes
        tracing::debug!(
            component = "config_server",
            event = "listing_routes",
            "About to call list_routes"
        );
        let list_data = self.list_routes();
        tracing::debug!(
            component = "config_server",
            event = "listed_routes",
            count = list_data.data.len(),
            "list_routes returned"
        );
        println!(
            "HTTPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // gRPC Routes
        let list_data = self.list_grpc_routes();
        println!(
            "GRPCRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // TCP Routes
        let list_data = self.list_tcp_routes();
        println!(
            "TCPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // UDP Routes
        let list_data = self.list_udp_routes();
        println!(
            "UDPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // TLS Routes
        let list_data = self.list_tls_routes();
        println!(
            "TLSRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // LinkSys
        let list_data = self.list_link_sys();
        println!(
            "LinkSys (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, link_sys) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(link_sys));
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

        // Plugin Metadata
        let list_data = self.list_plugin_metadata();
        println!(
            "PluginMetaData (count: {}, version: {}):",
            list_data.data.len(),
            list_data.resource_version
        );
        for (idx, metadata) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(metadata));
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

    /// Enable version fix mode for all caches
    pub fn enable_version_fix_mode(&self) {
        self.routes.enable_version_fix_mode();
        self.grpc_routes.enable_version_fix_mode();
        self.tcp_routes.enable_version_fix_mode();
        self.udp_routes.enable_version_fix_mode();
        self.tls_routes.enable_version_fix_mode();
        self.link_sys.enable_version_fix_mode();
        self.services.enable_version_fix_mode();
        self.endpoint_slices.enable_version_fix_mode();
        self.edgion_tls.enable_version_fix_mode();
        self.edgion_plugins.enable_version_fix_mode();
        self.plugin_metadata.enable_version_fix_mode();
        self.secrets.enable_version_fix_mode();
    }

    /// Set all caches to ready state
    pub fn set_ready(&self) {
        self.gateway_classes.set_ready();
        self.gateways.set_ready();
        self.edgion_gateway_configs.set_ready();
        self.routes.set_ready();
        self.grpc_routes.set_ready();
        self.tcp_routes.set_ready();
        self.udp_routes.set_ready();
        self.tls_routes.set_ready();
        self.link_sys.set_ready();
        self.services.set_ready();
        self.endpoint_slices.set_ready();
        self.edgion_tls.set_ready();
        self.edgion_plugins.set_ready();
        self.plugin_metadata.set_ready();
        self.secrets.set_ready();
    }

    // Helper methods for base conf resources

    /// List all GatewayClass resources
    pub fn list_gateway_classes(&self) -> ListData<GatewayClass> {
        self.gateway_classes.list_owned()
    }

    /// Watch GatewayClass resources
    pub fn watch_gateway_classes(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<GatewayClass>> {
        self.gateway_classes.watch(client_id, client_name, from_version)
    }

    /// List all Gateway resources
    pub fn list_gateways(&self) -> ListData<Gateway> {
        self.gateways.list_owned()
    }

    /// Watch Gateway resources
    pub fn watch_gateways(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<Gateway>> {
        self.gateways.watch(client_id, client_name, from_version)
    }

    /// List all EdgionGatewayConfig resources
    pub fn list_edgion_gateway_configs(&self) -> ListData<EdgionGatewayConfig> {
        self.edgion_gateway_configs.list_owned()
    }

    /// Watch EdgionGatewayConfig resources
    pub fn watch_edgion_gateway_configs(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<EdgionGatewayConfig>> {
        self.edgion_gateway_configs.watch(client_id, client_name, from_version)
    }
}
