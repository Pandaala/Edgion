use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;

use super::secret_ref::SecretRefManager;
use crate::core::conf_sync::cache_server::ServerCache;
use crate::core::conf_sync::traits::CacheEventDispatch;
use crate::core::conf_sync::types::{ListData, WatchResponse};
use crate::core::utils::format_resource_info;
use crate::types::prelude_resources::*;
use anyhow::Result;

/// Helper function to spawn a watch forwarder task that converts WatchResponse<T> to EventDataSimple
/// Includes server_id in the response for server instance change detection
fn spawn_watch_forwarder<T: serde::Serialize + Send + 'static>(
    mut receiver: mpsc::Receiver<WatchResponse<T>>,
    tx: mpsc::Sender<EventDataSimple>,
    type_name: &'static str,
    server_id: String,
) {
    tokio::spawn(async move {
        while let Some(response) = receiver.recv().await {
            let WatchResponse {
                events,
                sync_version,
                err,
            } = response;

            let events_json = match serde_json::to_string(&events) {
                Ok(json) => json,
                Err(e) => {
                    eprintln!("Failed to serialize {} events: {}", type_name, e);
                    continue;
                }
            };
            let event_data = EventDataSimple {
                data: events_json,
                sync_version,
                err,
                server_id: server_id.clone(),
            };
            if tx.send(event_data).await.is_err() {
                break;
            }
        }
    });
}

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
    /// Server instance ID, generated at startup (millisecond timestamp)
    /// Used by clients to detect server restarts/failovers
    /// Wrapped in RwLock to allow regeneration during relink
    server_id: RwLock<String>,

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
    pub sync_version: u64,
    pub server_id: String,
}

pub struct EventDataSimple {
    pub data: String,
    pub sync_version: u64,
    pub err: Option<String>,
    pub server_id: String,
}

impl ConfigServer {
    pub fn new(conf_sync_config: &crate::core::cli::config::ConfSyncConfig) -> Self {
        // Generate server_id using millisecond timestamp
        let server_id = Self::generate_server_id();

        tracing::info!(
            component = "config_server",
            server_id = %server_id,
            "ConfigServer initialized with server_id"
        );

        Self {
            server_id: RwLock::new(server_id),
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

    /// Generate a new server ID using millisecond timestamp
    fn generate_server_id() -> String {
        format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        )
    }

    /// Get the current server ID
    pub fn server_id(&self) -> String {
        self.server_id.read().unwrap().clone()
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
        // Note: Ready check is now handled by ConfCenter (config_server is Option)
        // If we're here, ConfCenter has already verified we're ready to serve

        let (data_json, sync_version) = match kind {
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
            sync_version,
            server_id: self.server_id(),
        })
    }

    pub fn watch(
        &self,
        kind: &ResourceKind,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Result<mpsc::Receiver<EventDataSimple>, String> {
        // Note: Ready check is now handled by ConfCenter (config_server is Option)
        // If we're here, ConfCenter has already verified we're ready to serve

        let (tx, rx) = mpsc::channel(100);
        let server_id = self.server_id();

        println!(
            "[ConfigCenter::watch] kind={:?} client_id={} client_name={} from_version={}",
            kind, client_id, client_name, from_version
        );

        match kind {
            ResourceKind::Unspecified => return Err("Resource kind unspecified".to_string()),
            ResourceKind::GatewayClass => {
                spawn_watch_forwarder(
                    self.watch_gateway_classes(client_id, client_name, from_version),
                    tx,
                    "GatewayClass",
                    server_id,
                );
            }
            ResourceKind::EdgionGatewayConfig => {
                spawn_watch_forwarder(
                    self.watch_edgion_gateway_configs(client_id, client_name, from_version),
                    tx,
                    "EdgionGatewayConfig",
                    server_id,
                );
            }
            ResourceKind::Gateway => {
                spawn_watch_forwarder(
                    self.watch_gateways(client_id, client_name, from_version),
                    tx,
                    "Gateway",
                    server_id,
                );
            }
            ResourceKind::HTTPRoute => {
                spawn_watch_forwarder(
                    self.watch_routes(client_id, client_name, from_version),
                    tx,
                    "HTTPRoute",
                    server_id,
                );
            }
            ResourceKind::GRPCRoute => {
                spawn_watch_forwarder(
                    self.watch_grpc_routes(client_id, client_name, from_version),
                    tx,
                    "GRPCRoute",
                    server_id,
                );
            }
            ResourceKind::TCPRoute => {
                spawn_watch_forwarder(
                    self.watch_tcp_routes(client_id, client_name, from_version),
                    tx,
                    "TCPRoute",
                    server_id,
                );
            }
            ResourceKind::UDPRoute => {
                spawn_watch_forwarder(
                    self.watch_udp_routes(client_id, client_name, from_version),
                    tx,
                    "UDPRoute",
                    server_id,
                );
            }
            ResourceKind::TLSRoute => {
                spawn_watch_forwarder(
                    self.watch_tls_routes(client_id, client_name, from_version),
                    tx,
                    "TLSRoute",
                    server_id,
                );
            }
            ResourceKind::LinkSys => {
                spawn_watch_forwarder(
                    self.watch_link_sys(client_id, client_name, from_version),
                    tx,
                    "LinkSys",
                    server_id,
                );
            }
            ResourceKind::PluginMetaData => {
                spawn_watch_forwarder(
                    self.watch_plugin_metadata(client_id, client_name, from_version),
                    tx,
                    "PluginMetaData",
                    server_id,
                );
            }
            ResourceKind::Service => {
                spawn_watch_forwarder(
                    self.watch_services(client_id, client_name, from_version),
                    tx,
                    "Service",
                    server_id,
                );
            }
            ResourceKind::EndpointSlice => {
                spawn_watch_forwarder(
                    self.watch_endpoint_slices(client_id, client_name, from_version),
                    tx,
                    "EndpointSlice",
                    server_id,
                );
            }
            ResourceKind::Endpoint => {
                spawn_watch_forwarder(
                    self.watch_endpoints(client_id, client_name, from_version),
                    tx,
                    "Endpoints",
                    server_id,
                );
            }
            ResourceKind::EdgionTls => {
                spawn_watch_forwarder(
                    self.watch_edgion_tls(client_id, client_name, from_version),
                    tx,
                    "EdgionTls",
                    server_id,
                );
            }
            ResourceKind::EdgionPlugins => {
                spawn_watch_forwarder(
                    self.watch_edgion_plugins(client_id, client_name, from_version),
                    tx,
                    "EdgionPlugins",
                    server_id,
                );
            }
            ResourceKind::EdgionStreamPlugins => {
                spawn_watch_forwarder(
                    self.watch_edgion_stream_plugins(client_id, client_name, from_version),
                    tx,
                    "EdgionStreamPlugins",
                    server_id,
                );
            }
            ResourceKind::ReferenceGrant => {
                spawn_watch_forwarder(
                    self.watch_reference_grants(client_id, client_name, from_version),
                    tx,
                    "ReferenceGrant",
                    server_id,
                );
            }
            ResourceKind::BackendTLSPolicy => {
                spawn_watch_forwarder(
                    self.watch_backend_tls_policies(client_id, client_name, from_version),
                    tx,
                    "BackendTLSPolicy",
                    server_id,
                );
            }
            ResourceKind::Secret => {
                spawn_watch_forwarder(
                    self.watch_secrets(client_id, client_name, from_version),
                    tx,
                    "Secret",
                    server_id,
                );
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
            list_data.sync_version
        );
        for (idx, gc) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(gc));
        }

        let list_data = self.list_edgion_gateway_configs();
        println!(
            "EdgionGatewayConfig (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, egwc) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(egwc));
        }

        let list_data = self.list_gateways();
        println!(
            "Gateway (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
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
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // gRPC Routes
        let list_data = self.list_grpc_routes();
        println!(
            "GRPCRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // TCP Routes
        let list_data = self.list_tcp_routes();
        println!(
            "TCPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // UDP Routes
        let list_data = self.list_udp_routes();
        println!(
            "UDPRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // TLS Routes
        let list_data = self.list_tls_routes();
        println!(
            "TLSRoutes (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, route) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(route));
        }

        // LinkSys
        let list_data = self.list_link_sys();
        println!(
            "LinkSys (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, link_sys) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(link_sys));
        }

        // Services
        let list_data = self.list_services();
        println!(
            "Services (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, svc) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(svc));
        }

        // Endpoint Slices
        let list_data = self.list_endpoint_slices();
        println!(
            "EndpointSlices (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, es) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(es));
        }

        // Edgion TLS
        let list_data = self.list_edgion_tls();
        println!(
            "EdgionTls (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, tls) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(tls));
        }

        // Edgion Plugins
        let list_data = self.list_edgion_plugins();
        println!(
            "EdgionPlugins (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, plugin) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(plugin));
        }

        // Plugin Metadata
        let list_data = self.list_plugin_metadata();
        println!(
            "PluginMetaData (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, metadata) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(metadata));
        }

        // Secrets
        let list_data = self.list_secrets();
        println!(
            "Secrets (count: {}, version: {}):",
            list_data.data.len(),
            list_data.sync_version
        );
        for (idx, secret) in list_data.data.iter().enumerate() {
            println!("  [{}] {}", idx, format_resource_info(secret));
        }

        println!("==========================\n");
    }

    /// Set all caches to ready state (deprecated: use set_cache_ready_by_kind instead)
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

    /// Set a specific cache to ready state by kind name
    /// Called when a K8s watcher receives InitDone event
    pub fn set_cache_ready_by_kind(&self, kind: &str) {
        match kind {
            "GatewayClass" => self.gateway_classes.set_ready(),
            "Gateway" => self.gateways.set_ready(),
            "EdgionGatewayConfig" => self.edgion_gateway_configs.set_ready(),
            "HTTPRoute" => self.routes.set_ready(),
            "GRPCRoute" => self.grpc_routes.set_ready(),
            "TCPRoute" => self.tcp_routes.set_ready(),
            "UDPRoute" => self.udp_routes.set_ready(),
            "TLSRoute" => self.tls_routes.set_ready(),
            "LinkSys" => self.link_sys.set_ready(),
            "Service" => self.services.set_ready(),
            "EndpointSlice" => self.endpoint_slices.set_ready(),
            "Endpoint" | "Endpoints" => self.endpoints.set_ready(),
            "EdgionTls" => self.edgion_tls.set_ready(),
            "EdgionPlugins" => self.edgion_plugins.set_ready(),
            "EdgionStreamPlugins" => self.edgion_stream_plugins.set_ready(),
            "ReferenceGrant" => self.reference_grants.set_ready(),
            "BackendTLSPolicy" => self.backend_tls_policies.set_ready(),
            "PluginMetaData" => self.plugin_metadata.set_ready(),
            "Secret" => self.secrets.set_ready(),
            _ => {
                tracing::warn!(
                    component = "config_server",
                    kind = kind,
                    "Unknown resource kind for set_cache_ready_by_kind"
                );
            }
        }
        tracing::info!(component = "config_server", kind = kind, "Cache marked as ready");
    }

    /// Check if all individual caches are ready (internal check)
    pub fn is_each_cache_ready(&self) -> bool {
        self.gateway_classes.is_ready()
            && self.gateways.is_ready()
            && self.edgion_gateway_configs.is_ready()
            && self.routes.is_ready()
            && self.grpc_routes.is_ready()
            && self.tcp_routes.is_ready()
            && self.udp_routes.is_ready()
            && self.tls_routes.is_ready()
            && self.link_sys.is_ready()
            && self.services.is_ready()
            && self.endpoint_slices.is_ready()
            && self.endpoints.is_ready()
            && self.edgion_tls.is_ready()
            && self.edgion_plugins.is_ready()
            && self.edgion_stream_plugins.is_ready()
            && self.reference_grants.is_ready()
            && self.backend_tls_policies.is_ready()
            && self.plugin_metadata.is_ready()
            && self.secrets.is_ready()
    }

    /// Get list of caches that are not ready yet
    pub fn not_ready_caches(&self) -> Vec<&'static str> {
        let mut not_ready = Vec::new();
        if !self.gateway_classes.is_ready() {
            not_ready.push("GatewayClass");
        }
        if !self.gateways.is_ready() {
            not_ready.push("Gateway");
        }
        if !self.edgion_gateway_configs.is_ready() {
            not_ready.push("EdgionGatewayConfig");
        }
        if !self.routes.is_ready() {
            not_ready.push("HTTPRoute");
        }
        if !self.grpc_routes.is_ready() {
            not_ready.push("GRPCRoute");
        }
        if !self.tcp_routes.is_ready() {
            not_ready.push("TCPRoute");
        }
        if !self.udp_routes.is_ready() {
            not_ready.push("UDPRoute");
        }
        if !self.tls_routes.is_ready() {
            not_ready.push("TLSRoute");
        }
        if !self.link_sys.is_ready() {
            not_ready.push("LinkSys");
        }
        if !self.services.is_ready() {
            not_ready.push("Service");
        }
        if !self.endpoint_slices.is_ready() {
            not_ready.push("EndpointSlice");
        }
        if !self.endpoints.is_ready() {
            not_ready.push("Endpoints");
        }
        if !self.edgion_tls.is_ready() {
            not_ready.push("EdgionTls");
        }
        if !self.edgion_plugins.is_ready() {
            not_ready.push("EdgionPlugins");
        }
        if !self.edgion_stream_plugins.is_ready() {
            not_ready.push("EdgionStreamPlugins");
        }
        if !self.reference_grants.is_ready() {
            not_ready.push("ReferenceGrant");
        }
        if !self.backend_tls_policies.is_ready() {
            not_ready.push("BackendTLSPolicy");
        }
        if !self.plugin_metadata.is_ready() {
            not_ready.push("PluginMetaData");
        }
        if !self.secrets.is_ready() {
            not_ready.push("Secret");
        }
        not_ready
    }

    /// Set all caches to not ready state
    /// Used during relink to prevent serving stale data
    pub fn set_all_caches_not_ready(&self) {
        self.gateway_classes.set_not_ready();
        self.gateways.set_not_ready();
        self.edgion_gateway_configs.set_not_ready();
        self.routes.set_not_ready();
        self.grpc_routes.set_not_ready();
        self.tcp_routes.set_not_ready();
        self.udp_routes.set_not_ready();
        self.tls_routes.set_not_ready();
        self.link_sys.set_not_ready();
        self.services.set_not_ready();
        self.endpoint_slices.set_not_ready();
        self.endpoints.set_not_ready();
        self.edgion_tls.set_not_ready();
        self.edgion_plugins.set_not_ready();
        self.edgion_stream_plugins.set_not_ready();
        self.reference_grants.set_not_ready();
        self.backend_tls_policies.set_not_ready();
        self.plugin_metadata.set_not_ready();
        self.secrets.set_not_ready();
    }

    /// Clear all cache data
    /// Used during relink to remove stale data
    pub fn clear_all_caches(&self) {
        self.gateway_classes.clear();
        self.gateways.clear();
        self.edgion_gateway_configs.clear();
        self.routes.clear();
        self.grpc_routes.clear();
        self.tcp_routes.clear();
        self.udp_routes.clear();
        self.tls_routes.clear();
        self.link_sys.clear();
        self.services.clear();
        self.endpoint_slices.clear();
        self.endpoints.clear();
        self.edgion_tls.clear();
        self.edgion_plugins.clear();
        self.edgion_stream_plugins.clear();
        self.reference_grants.clear();
        self.backend_tls_policies.clear();
        self.plugin_metadata.clear();
        self.secrets.clear();
        // Also clear secret reference manager
        self.secret_ref_manager.clear();
    }

    /// Regenerate server ID
    /// Used during relink to trigger clients to re-sync
    pub fn regenerate_server_id(&self) {
        let old_id = self.server_id();
        let new_id = Self::generate_server_id();
        tracing::info!(
            component = "config_server",
            old_server_id = %old_id,
            new_server_id = %new_id,
            "Regenerating server_id for relink"
        );
        *self.server_id.write().unwrap() = new_id;
    }

    /// Reset for relink: clear all caches and regenerate server ID
    /// Called when 410 Gone or leader re-election requires full state reset
    pub fn reset_for_relink(&self) {
        tracing::info!(component = "config_server", "Resetting ConfigServer for relink");

        // 1. Set all caches to not ready
        self.set_all_caches_not_ready();

        // 2. Clear all cache data
        self.clear_all_caches();

        // 3. Regenerate server ID (triggers client re-sync)
        self.regenerate_server_id();

        tracing::info!(
            component = "config_server",
            server_id = %self.server_id(),
            "ConfigServer reset complete"
        );
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
