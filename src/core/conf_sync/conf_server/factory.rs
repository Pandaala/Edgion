//! ServerCacheFactory - manages all ServerCache instances via HashMap
//!
//! This factory creates and manages all ServerCache<T> instances,
//! storing them as Arc<dyn ServerCacheObj> to enable generic access.

use std::collections::HashMap;
use std::sync::Arc;

use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

use crate::core::cli::config::ConfSyncConfig;
use crate::core::conf_sync::ServerCache;
use crate::types::prelude_resources::*;

use super::traits::ServerCacheObj;

/// Resource kind names (matching ResourceKind enum names)
pub mod kind_names {
    pub const GATEWAY_CLASS: &str = "GatewayClass";
    pub const GATEWAY: &str = "Gateway";
    pub const EDGION_GATEWAY_CONFIG: &str = "EdgionGatewayConfig";
    pub const HTTP_ROUTE: &str = "HTTPRoute";
    pub const GRPC_ROUTE: &str = "GRPCRoute";
    pub const TCP_ROUTE: &str = "TCPRoute";
    pub const UDP_ROUTE: &str = "UDPRoute";
    pub const TLS_ROUTE: &str = "TLSRoute";
    pub const LINK_SYS: &str = "LinkSys";
    pub const SERVICE: &str = "Service";
    pub const ENDPOINT_SLICE: &str = "EndpointSlice";
    pub const ENDPOINTS: &str = "Endpoints";
    pub const EDGION_TLS: &str = "EdgionTls";
    pub const EDGION_PLUGINS: &str = "EdgionPlugins";
    pub const EDGION_STREAM_PLUGINS: &str = "EdgionStreamPlugins";
    pub const REFERENCE_GRANT: &str = "ReferenceGrant";
    pub const BACKEND_TLS_POLICY: &str = "BackendTLSPolicy";
    pub const PLUGIN_METADATA: &str = "PluginMetaData";
    pub const SECRET: &str = "Secret";
}

/// Factory that manages all ServerCache instances
///
/// Provides a unified interface to access different resource caches
/// without knowing the concrete type at compile time.
pub struct ServerCacheFactory {
    /// Map from kind name to cache instance
    caches: HashMap<String, Arc<dyn ServerCacheObj>>,

    // Keep typed references for special operations (apply_change, etc.)
    // These are the same instances as in the HashMap, just with typed access
    pub gateway_classes: Arc<ServerCache<GatewayClass>>,
    pub gateways: Arc<ServerCache<Gateway>>,
    pub edgion_gateway_configs: Arc<ServerCache<EdgionGatewayConfig>>,
    pub http_routes: Arc<ServerCache<HTTPRoute>>,
    pub grpc_routes: Arc<ServerCache<GRPCRoute>>,
    pub tcp_routes: Arc<ServerCache<TCPRoute>>,
    pub udp_routes: Arc<ServerCache<UDPRoute>>,
    pub tls_routes: Arc<ServerCache<TLSRoute>>,
    pub link_sys: Arc<ServerCache<LinkSys>>,
    pub services: Arc<ServerCache<Service>>,
    pub endpoint_slices: Arc<ServerCache<EndpointSlice>>,
    pub endpoints: Arc<ServerCache<Endpoints>>,
    pub edgion_tls: Arc<ServerCache<EdgionTls>>,
    pub edgion_plugins: Arc<ServerCache<EdgionPlugins>>,
    pub edgion_stream_plugins: Arc<ServerCache<EdgionStreamPlugins>>,
    pub reference_grants: Arc<ServerCache<ReferenceGrant>>,
    pub backend_tls_policies: Arc<ServerCache<BackendTLSPolicy>>,
    pub plugin_metadata: Arc<ServerCache<PluginMetaData>>,
    pub secrets: Arc<ServerCache<Secret>>,
}

impl ServerCacheFactory {
    /// Create a new factory with all caches initialized
    pub fn new(config: &ConfSyncConfig) -> Self {
        // Create all typed caches
        let gateway_classes = Arc::new(ServerCache::new(config.gateway_classes_capacity));
        let gateways = Arc::new(ServerCache::new(config.gateways_capacity));
        let edgion_gateway_configs = Arc::new(ServerCache::new(config.edgion_gateway_configs_capacity));
        let http_routes = Arc::new(ServerCache::new(config.routes_capacity));
        let grpc_routes = Arc::new(ServerCache::new(config.grpc_routes_capacity));
        let tcp_routes = Arc::new(ServerCache::new(config.tcp_routes_capacity));
        let udp_routes = Arc::new(ServerCache::new(config.udp_routes_capacity));
        let tls_routes = Arc::new(ServerCache::new(config.tls_routes_capacity));
        let link_sys = Arc::new(ServerCache::new(config.link_sys_capacity));
        let services = Arc::new(ServerCache::new(config.services_capacity));
        let endpoint_slices = Arc::new(ServerCache::new(config.endpoint_slices_capacity));
        let endpoints = Arc::new(ServerCache::new(config.endpoints_capacity));
        let edgion_tls = Arc::new(ServerCache::new(config.edgion_tls_capacity));
        let edgion_plugins = Arc::new(ServerCache::new(config.edgion_plugins_capacity));
        let edgion_stream_plugins = Arc::new(ServerCache::new(config.edgion_stream_plugins_capacity));
        let reference_grants = Arc::new(ServerCache::new(config.reference_grants_capacity));
        let backend_tls_policies = Arc::new(ServerCache::new(config.backend_tls_policies_capacity));
        let plugin_metadata = Arc::new(ServerCache::new(config.plugin_metadata_capacity));
        let secrets = Arc::new(ServerCache::new(config.secrets_capacity));

        // Build the HashMap with Arc<dyn ServerCacheObj>
        let mut caches: HashMap<String, Arc<dyn ServerCacheObj>> = HashMap::new();

        caches.insert(kind_names::GATEWAY_CLASS.to_string(), gateway_classes.clone());
        caches.insert(kind_names::GATEWAY.to_string(), gateways.clone());
        caches.insert(kind_names::EDGION_GATEWAY_CONFIG.to_string(), edgion_gateway_configs.clone());
        caches.insert(kind_names::HTTP_ROUTE.to_string(), http_routes.clone());
        caches.insert(kind_names::GRPC_ROUTE.to_string(), grpc_routes.clone());
        caches.insert(kind_names::TCP_ROUTE.to_string(), tcp_routes.clone());
        caches.insert(kind_names::UDP_ROUTE.to_string(), udp_routes.clone());
        caches.insert(kind_names::TLS_ROUTE.to_string(), tls_routes.clone());
        caches.insert(kind_names::LINK_SYS.to_string(), link_sys.clone());
        caches.insert(kind_names::SERVICE.to_string(), services.clone());
        caches.insert(kind_names::ENDPOINT_SLICE.to_string(), endpoint_slices.clone());
        caches.insert(kind_names::ENDPOINTS.to_string(), endpoints.clone());
        caches.insert(kind_names::EDGION_TLS.to_string(), edgion_tls.clone());
        caches.insert(kind_names::EDGION_PLUGINS.to_string(), edgion_plugins.clone());
        caches.insert(kind_names::EDGION_STREAM_PLUGINS.to_string(), edgion_stream_plugins.clone());
        caches.insert(kind_names::REFERENCE_GRANT.to_string(), reference_grants.clone());
        caches.insert(kind_names::BACKEND_TLS_POLICY.to_string(), backend_tls_policies.clone());
        caches.insert(kind_names::PLUGIN_METADATA.to_string(), plugin_metadata.clone());
        caches.insert(kind_names::SECRET.to_string(), secrets.clone());

        Self {
            caches,
            gateway_classes,
            gateways,
            edgion_gateway_configs,
            http_routes,
            grpc_routes,
            tcp_routes,
            udp_routes,
            tls_routes,
            link_sys,
            services,
            endpoint_slices,
            endpoints,
            edgion_tls,
            edgion_plugins,
            edgion_stream_plugins,
            reference_grants,
            backend_tls_policies,
            plugin_metadata,
            secrets,
        }
    }

    /// Get a cache by kind name
    ///
    /// Returns None if the kind is not found
    pub fn get_cache(&self, kind: &str) -> Option<Arc<dyn ServerCacheObj>> {
        self.caches.get(kind).cloned()
    }

    /// Get all registered kind names
    pub fn all_kinds(&self) -> Vec<&str> {
        self.caches.keys().map(|s| s.as_str()).collect()
    }

    /// Set all caches to ready state
    pub fn set_all_ready(&self) {
        for cache in self.caches.values() {
            cache.set_ready();
        }
    }

    /// Set all caches to not ready state
    pub fn set_all_not_ready(&self) {
        for cache in self.caches.values() {
            cache.set_not_ready();
        }
    }

    /// Clear all caches
    pub fn clear_all(&self) {
        for cache in self.caches.values() {
            cache.clear();
        }
    }

    /// Check if all caches are ready
    pub fn is_all_ready(&self) -> bool {
        self.caches.values().all(|cache| cache.is_ready())
    }

    /// Get list of caches that are not ready
    pub fn not_ready_kinds(&self) -> Vec<&str> {
        self.caches
            .iter()
            .filter(|(_, cache)| !cache.is_ready())
            .map(|(kind, _)| kind.as_str())
            .collect()
    }

    /// Set a specific cache to ready state by kind name
    pub fn set_cache_ready_by_kind(&self, kind: &str) {
        if let Some(cache) = self.caches.get(kind) {
            cache.set_ready();
            tracing::info!(component = "cache_factory", kind = kind, "Cache marked as ready");
        } else {
            tracing::warn!(
                component = "cache_factory",
                kind = kind,
                "Unknown resource kind for set_cache_ready_by_kind"
            );
        }
    }
}
