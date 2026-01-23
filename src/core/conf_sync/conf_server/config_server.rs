//! New ConfigServer implementation using ServerCacheFactory
//!
//! This replaces the old ConfigServer with a simplified design that uses
//! ServerCacheFactory to manage all caches via trait objects.

use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;

use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

use crate::core::cli::config::ConfSyncConfig;
use crate::core::conf_mgr::conf_center::EndpointMode;
use crate::types::prelude_resources::*;

use super::factory::{kind_names, ServerCacheFactory};
use super::traits::ServerCacheObj;

/// Simplified list data response (JSON-serialized)
pub struct ListDataSimple {
    pub data: String,
    pub sync_version: u64,
    pub server_id: String,
}

/// Simplified event data for watch responses
pub struct EventDataSimple {
    pub data: String,
    pub sync_version: u64,
    pub err: Option<String>,
    pub server_id: String,
}

/// Resource item enum for apply_change operations
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

/// ConfigServer manages all resource caches and provides list/watch operations
///
/// This new implementation uses ServerCacheFactory to manage caches,
/// significantly simplifying the list and watch methods.
///
/// For backward compatibility, the typed cache fields (routes, secrets, etc.)
/// are publicly accessible. Prefer using factory() for new code.
pub struct ConfigServer {
    /// Server instance ID, generated at startup
    server_id: RwLock<String>,

    /// Endpoint discovery mode (K8s resolved mode)
    endpoint_mode: RwLock<Option<EndpointMode>>,

    /// Factory that manages all caches
    factory: ServerCacheFactory,

    // ==================== Backward Compatibility Fields ====================
    // These fields provide direct access to typed caches for existing code
    // that accesses cs.routes, cs.secrets, etc.

    /// HTTP routes cache
    pub routes: Arc<crate::core::conf_sync::ServerCache<HTTPRoute>>,
    /// gRPC routes cache
    pub grpc_routes: Arc<crate::core::conf_sync::ServerCache<GRPCRoute>>,
    /// TCP routes cache
    pub tcp_routes: Arc<crate::core::conf_sync::ServerCache<TCPRoute>>,
    /// UDP routes cache
    pub udp_routes: Arc<crate::core::conf_sync::ServerCache<UDPRoute>>,
    /// TLS routes cache
    pub tls_routes: Arc<crate::core::conf_sync::ServerCache<TLSRoute>>,
    /// Gateway classes cache
    pub gateway_classes: Arc<crate::core::conf_sync::ServerCache<GatewayClass>>,
    /// Gateways cache
    pub gateways: Arc<crate::core::conf_sync::ServerCache<Gateway>>,
    /// EdgionGatewayConfigs cache
    pub edgion_gateway_configs: Arc<crate::core::conf_sync::ServerCache<EdgionGatewayConfig>>,
    /// LinkSys cache
    pub link_sys: Arc<crate::core::conf_sync::ServerCache<LinkSys>>,
    /// Services cache
    pub services: Arc<crate::core::conf_sync::ServerCache<Service>>,
    /// EndpointSlices cache
    pub endpoint_slices: Arc<crate::core::conf_sync::ServerCache<EndpointSlice>>,
    /// Endpoints cache
    pub endpoints: Arc<crate::core::conf_sync::ServerCache<Endpoints>>,
    /// EdgionTls cache
    pub edgion_tls: Arc<crate::core::conf_sync::ServerCache<EdgionTls>>,
    /// EdgionPlugins cache
    pub edgion_plugins: Arc<crate::core::conf_sync::ServerCache<EdgionPlugins>>,
    /// EdgionStreamPlugins cache
    pub edgion_stream_plugins: Arc<crate::core::conf_sync::ServerCache<EdgionStreamPlugins>>,
    /// ReferenceGrants cache
    pub reference_grants: Arc<crate::core::conf_sync::ServerCache<ReferenceGrant>>,
    /// BackendTLSPolicies cache
    pub backend_tls_policies: Arc<crate::core::conf_sync::ServerCache<BackendTLSPolicy>>,
    /// PluginMetaData cache
    pub plugin_metadata: Arc<crate::core::conf_sync::ServerCache<PluginMetaData>>,
    /// Secrets cache
    pub secrets: Arc<crate::core::conf_sync::ServerCache<Secret>>,
}

impl ConfigServer {
    /// Create a new ConfigServer
    pub fn new(conf_sync_config: &ConfSyncConfig) -> Self {
        let server_id = Self::generate_server_id();

        tracing::info!(
            component = "config_server",
            server_id = %server_id,
            "ConfigServer initialized with server_id"
        );

        let factory = ServerCacheFactory::new(conf_sync_config);

        // Clone Arc references for backward compatibility fields
        Self {
            server_id: RwLock::new(server_id),
            endpoint_mode: RwLock::new(None),
            // Clone cache references for backward compatibility
            routes: factory.http_routes.clone(),
            grpc_routes: factory.grpc_routes.clone(),
            tcp_routes: factory.tcp_routes.clone(),
            udp_routes: factory.udp_routes.clone(),
            tls_routes: factory.tls_routes.clone(),
            gateway_classes: factory.gateway_classes.clone(),
            gateways: factory.gateways.clone(),
            edgion_gateway_configs: factory.edgion_gateway_configs.clone(),
            link_sys: factory.link_sys.clone(),
            services: factory.services.clone(),
            endpoint_slices: factory.endpoint_slices.clone(),
            endpoints: factory.endpoints.clone(),
            edgion_tls: factory.edgion_tls.clone(),
            edgion_plugins: factory.edgion_plugins.clone(),
            edgion_stream_plugins: factory.edgion_stream_plugins.clone(),
            reference_grants: factory.reference_grants.clone(),
            backend_tls_policies: factory.backend_tls_policies.clone(),
            plugin_metadata: factory.plugin_metadata.clone(),
            secrets: factory.secrets.clone(),
            factory,
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

    /// Set endpoint discovery mode
    pub fn set_endpoint_mode(&self, mode: EndpointMode) {
        *self.endpoint_mode.write().unwrap() = Some(mode);
    }

    /// Get endpoint discovery mode
    pub fn endpoint_mode(&self) -> Option<EndpointMode> {
        *self.endpoint_mode.read().unwrap()
    }

    // ==================== Simplified list/watch via Factory ====================

    /// Convert ResourceKind to cache kind name
    fn kind_to_name(kind: &ResourceKind) -> Option<&'static str> {
        match kind {
            ResourceKind::Unspecified => None,
            ResourceKind::GatewayClass => Some(kind_names::GATEWAY_CLASS),
            ResourceKind::EdgionGatewayConfig => Some(kind_names::EDGION_GATEWAY_CONFIG),
            ResourceKind::Gateway => Some(kind_names::GATEWAY),
            ResourceKind::HTTPRoute => Some(kind_names::HTTP_ROUTE),
            ResourceKind::GRPCRoute => Some(kind_names::GRPC_ROUTE),
            ResourceKind::TCPRoute => Some(kind_names::TCP_ROUTE),
            ResourceKind::UDPRoute => Some(kind_names::UDP_ROUTE),
            ResourceKind::TLSRoute => Some(kind_names::TLS_ROUTE),
            ResourceKind::LinkSys => Some(kind_names::LINK_SYS),
            ResourceKind::Service => Some(kind_names::SERVICE),
            ResourceKind::EndpointSlice => Some(kind_names::ENDPOINT_SLICE),
            ResourceKind::Endpoint => Some(kind_names::ENDPOINTS),
            ResourceKind::EdgionTls => Some(kind_names::EDGION_TLS),
            ResourceKind::EdgionPlugins => Some(kind_names::EDGION_PLUGINS),
            ResourceKind::EdgionStreamPlugins => Some(kind_names::EDGION_STREAM_PLUGINS),
            ResourceKind::ReferenceGrant => Some(kind_names::REFERENCE_GRANT),
            ResourceKind::BackendTLSPolicy => Some(kind_names::BACKEND_TLS_POLICY),
            ResourceKind::PluginMetaData => Some(kind_names::PLUGIN_METADATA),
            ResourceKind::Secret => Some(kind_names::SECRET),
        }
    }

    /// List resources by kind (simplified using factory)
    pub fn list(&self, kind: &ResourceKind) -> Result<ListDataSimple, String> {
        let kind_name = Self::kind_to_name(kind).ok_or("Resource kind unspecified")?;

        let cache = self
            .factory
            .get_cache(kind_name)
            .ok_or_else(|| format!("Unknown resource kind: {}", kind_name))?;

        let (data_json, sync_version) = cache.list_json()?;

        Ok(ListDataSimple {
            data: data_json,
            sync_version,
            server_id: self.server_id(),
        })
    }

    /// Watch resources by kind (simplified using factory)
    pub fn watch(
        &self,
        kind: &ResourceKind,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Result<mpsc::Receiver<EventDataSimple>, String> {
        let kind_name = Self::kind_to_name(kind).ok_or("Resource kind unspecified")?;

        let cache = self
            .factory
            .get_cache(kind_name)
            .ok_or_else(|| format!("Unknown resource kind: {}", kind_name))?;

        tracing::info!(
            component = "config_server",
            kind = kind_name,
            client_id = %client_id,
            client_name = %client_name,
            from_version = from_version,
            "Starting watch"
        );

        let simple_rx = cache.watch_json(client_id, client_name, from_version);
        let server_id = self.server_id();

        // Convert WatchResponseSimple to EventDataSimple
        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            let mut simple_rx = simple_rx;
            while let Some(response) = simple_rx.recv().await {
                let event_data = EventDataSimple {
                    data: response.data,
                    sync_version: response.sync_version,
                    err: response.err,
                    server_id: server_id.clone(),
                };

                if tx.send(event_data).await.is_err() {
                    break;
                }
            }
        });

        Ok(rx)
    }

    /// Get a cache by kind name (for external registration)
    pub fn get_cache(&self, kind: &str) -> Option<Arc<dyn ServerCacheObj>> {
        self.factory.get_cache(kind)
    }

    /// Get the factory (for typed access to specific caches)
    pub fn factory(&self) -> &ServerCacheFactory {
        &self.factory
    }

    // ==================== Cache State Management ====================

    /// Set a specific cache to ready state by kind name
    pub fn set_cache_ready_by_kind(&self, kind: &str) {
        // Handle "Endpoint" -> "Endpoints" mapping
        let normalized_kind = if kind == "Endpoint" {
            kind_names::ENDPOINTS
        } else {
            kind
        };
        self.factory.set_cache_ready_by_kind(normalized_kind);
    }

    /// Check if all caches are ready
    pub fn is_each_cache_ready(&self) -> bool {
        // Check base caches
        let base_ready = self.factory.is_all_ready();

        // Handle endpoint mode
        let endpoint_ready = match self.endpoint_mode() {
            Some(EndpointMode::Endpoint) => self.factory.endpoints.is_ready(),
            Some(EndpointMode::EndpointSlice) | None => self.factory.endpoint_slices.is_ready(),
            Some(EndpointMode::Auto) => {
                // Auto should be resolved before readiness checks
                tracing::warn!("EndpointMode::Auto should be resolved before cache checks");
                false
            }
        };

        base_ready && endpoint_ready
    }

    /// Get list of caches that are not ready
    pub fn not_ready_caches(&self) -> Vec<&'static str> {
        self.factory
            .not_ready_kinds()
            .into_iter()
            .map(|s| {
                // Convert to static str for compatibility
                match s {
                    "GatewayClass" => "GatewayClass",
                    "Gateway" => "Gateway",
                    "EdgionGatewayConfig" => "EdgionGatewayConfig",
                    "HTTPRoute" => "HTTPRoute",
                    "GRPCRoute" => "GRPCRoute",
                    "TCPRoute" => "TCPRoute",
                    "UDPRoute" => "UDPRoute",
                    "TLSRoute" => "TLSRoute",
                    "LinkSys" => "LinkSys",
                    "Service" => "Service",
                    "EndpointSlice" => "EndpointSlice",
                    "Endpoints" => "Endpoints",
                    "EdgionTls" => "EdgionTls",
                    "EdgionPlugins" => "EdgionPlugins",
                    "EdgionStreamPlugins" => "EdgionStreamPlugins",
                    "ReferenceGrant" => "ReferenceGrant",
                    "BackendTLSPolicy" => "BackendTLSPolicy",
                    "PluginMetaData" => "PluginMetaData",
                    "Secret" => "Secret",
                    _ => "Unknown",
                }
            })
            .collect()
    }

    /// Set all caches to not ready state
    pub fn set_all_caches_not_ready(&self) {
        self.factory.set_all_not_ready();
    }

    /// Clear all cache data
    pub fn clear_all_caches(&self) {
        self.factory.clear_all();
    }

    /// Regenerate server ID (for relink)
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

    /// Reset for relink
    pub fn reset_for_relink(&self) {
        tracing::info!(component = "config_server", "Resetting ConfigServer for relink");
        self.set_all_caches_not_ready();
        self.clear_all_caches();
        self.regenerate_server_id();
        tracing::info!(
            component = "config_server",
            server_id = %self.server_id(),
            "ConfigServer reset complete"
        );
    }

    /// Set all caches to ready (deprecated: use set_cache_ready_by_kind instead)
    pub fn set_ready(&self) {
        self.factory.set_all_ready();
    }

}
