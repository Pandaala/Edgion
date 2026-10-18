use crate::backends::{create_ep_slice_handler, create_service_handler, get_service_discovery_coordinator};
use crate::conf_sync::cache_client::{ClientCache, DynClientCache};
use crate::routes::create_route_manager_handler;
use anyhow::Result;
use edgion_common::conf_sync::types::ListData;
use edgion_resources::prelude_resources::*;
use edgion_resources::resource_defs::get_resource_info;
use edgion_resources::{all_resource_type_names, ResourceKind};
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

use super::scoped_handler::{ScopedHandler, SharedScopeRegistry};

pub struct ConfigClient {
    /// Current server_id from Controller (updated on each list/watch response)
    current_server_id: RwLock<String>,
    gateway_scope: Option<String>,
    gateway_uid: Option<String>,
    shared_scope: Arc<SharedScopeRegistry>,
    gateway_classes: ClientCache<GatewayClass>,
    gateways: ClientCache<Gateway>,
    edgion_gateway_configs: ClientCache<EdgionGatewayConfig>,
    routes: ClientCache<HTTPRoute>,
    grpc_routes: ClientCache<GRPCRoute>,
    tcp_routes: ClientCache<TCPRoute>,
    udp_routes: ClientCache<UDPRoute>,
    tls_routes: ClientCache<TLSRoute>,
    link_sys: ClientCache<LinkSys>,
    services: ClientCache<Service>,
    endpoint_slices: ClientCache<EndpointSlice>,
    edgion_tls: ClientCache<EdgionTls>,
    edgion_plugins: ClientCache<EdgionPlugins>,
    edgion_stream_plugins: ClientCache<EdgionStreamPlugins>,
    backend_tls_policies: ClientCache<BackendTLSPolicy>,
    edgion_config_data: ClientCache<EdgionConfigData>,
    edgion_backends: ClientCache<EdgionBackend>,
    edgion_acme: ClientCache<EdgionAcme>,
    edgion_backend_traffic_policy: ClientCache<EdgionBackendTrafficPolicy>,
    // secrets: ClientCache<Secret>,  // Secret now follows related resources
    /// Optional cache-field whitelist used for readiness checks.
    /// When set, `is_ready()` only waits for these caches.
    required_cache_names: RwLock<Option<HashSet<String>>>,
}

impl ConfigClient {
    /// Let publication tests hold a real cache lock without changing the runtime API.
    #[cfg(test)]
    pub(super) fn publication_test_cache(&self) -> ClientCache<Gateway> {
        self.gateways.clone()
    }

    /// Create a new ConfigClient
    ///
    /// # Arguments
    /// * `client_id` - Unique identifier for this client
    /// * `client_name` - Human-readable name for this client
    pub fn new(client_id: String, client_name: String) -> Self {
        Self::new_scoped(client_id, client_name, None, None)
    }

    pub fn new_scoped(
        client_id: String,
        client_name: String,
        gateway_scope: Option<String>,
        gateway_uid: Option<String>,
    ) -> Self {
        let shared_scope = Arc::new(SharedScopeRegistry::default());
        let routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let route_handler = create_route_manager_handler();
        routes_cache.set_conf_processor(ScopedHandler::wrap(
            route_handler,
            gateway_scope.clone(),
            Arc::clone(&shared_scope),
        ));

        let service_discovery = Arc::clone(get_service_discovery_coordinator());
        let services_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let service_handler = create_service_handler(Arc::clone(&service_discovery));
        services_cache.set_conf_processor(service_handler);

        let endpoint_slices_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let ep_slice_handler = create_ep_slice_handler(service_discovery);
        endpoint_slices_cache.set_conf_processor(ep_slice_handler);

        let plugins_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let plugin_handler = crate::plugins::http::create_plugin_handler();
        plugins_cache.set_conf_processor(plugin_handler);

        let tcp_routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let tcp_route_handler = crate::routes::tcp::create_tcp_route_handler();
        tcp_routes_cache.set_conf_processor(ScopedHandler::wrap(
            tcp_route_handler,
            gateway_scope.clone(),
            Arc::clone(&shared_scope),
        ));

        let udp_routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let udp_route_handler = crate::routes::udp::create_udp_route_handler();
        udp_routes_cache.set_conf_processor(ScopedHandler::wrap(
            udp_route_handler,
            gateway_scope.clone(),
            Arc::clone(&shared_scope),
        ));

        let grpc_routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let grpc_route_handler = crate::routes::grpc::create_grpc_route_handler();
        grpc_routes_cache.set_conf_processor(ScopedHandler::wrap(
            grpc_route_handler,
            gateway_scope.clone(),
            Arc::clone(&shared_scope),
        ));

        let tls_routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let tls_route_handler = crate::routes::tls::create_tls_route_handler();
        tls_routes_cache.set_conf_processor(ScopedHandler::wrap(
            tls_route_handler,
            gateway_scope.clone(),
            Arc::clone(&shared_scope),
        ));

        let edgion_tls_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let tls_handler = crate::tls::create_tls_handler();
        edgion_tls_cache.set_conf_processor(ScopedHandler::wrap(
            tls_handler,
            gateway_scope.clone(),
            Arc::clone(&shared_scope),
        ));

        let stream_plugins_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let stream_plugin_handler = crate::plugins::stream::create_stream_plugin_handler();
        stream_plugins_cache.set_conf_processor(stream_plugin_handler);

        let backend_tls_policies_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let backend_tls_policy_handler = crate::backends::create_backend_tls_policy_handler();
        backend_tls_policies_cache.set_conf_processor(backend_tls_policy_handler);

        let gateway_classes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let gateway_class_handler = crate::config::gateway_class::create_gateway_class_handler();
        gateway_classes_cache.set_conf_processor(gateway_class_handler);

        let gateways_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let gateway_handler = crate::runtime::create_scoped_gateway_handler(
            gateway_scope.clone(),
            gateway_uid.clone(),
            Arc::clone(&shared_scope),
        );
        gateways_cache.set_conf_processor(gateway_handler);

        let edgion_gateway_configs_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let edgion_gateway_config_handler = crate::config::edgion_gateway::create_edgion_gateway_config_handler();
        edgion_gateway_configs_cache.set_conf_processor(edgion_gateway_config_handler);

        let edgion_acme_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let acme_handler = crate::services::acme::create_acme_handler();
        edgion_acme_cache.set_conf_processor(ScopedHandler::wrap(
            acme_handler,
            gateway_scope.clone(),
            Arc::clone(&shared_scope),
        ));

        let edgion_backend_traffic_policy_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let backend_traffic_policy_handler = crate::backends::policy::create_backend_traffic_policy_handler();
        edgion_backend_traffic_policy_cache.set_conf_processor(backend_traffic_policy_handler);

        Self {
            current_server_id: RwLock::new(String::new()),
            gateway_scope,
            gateway_uid,
            shared_scope,
            gateway_classes: gateway_classes_cache,
            gateways: gateways_cache,
            edgion_gateway_configs: edgion_gateway_configs_cache,
            routes: routes_cache,
            grpc_routes: grpc_routes_cache,
            tcp_routes: tcp_routes_cache,
            udp_routes: udp_routes_cache,
            tls_routes: tls_routes_cache,
            link_sys: {
                let link_sys_cache = ClientCache::new(client_id.clone(), client_name.clone());
                let link_sys_handler = crate::link_sys::create_link_sys_handler();
                link_sys_cache.set_conf_processor(link_sys_handler);
                link_sys_cache
            },
            services: services_cache,
            endpoint_slices: endpoint_slices_cache,
            edgion_tls: edgion_tls_cache,
            edgion_plugins: plugins_cache,
            edgion_stream_plugins: stream_plugins_cache,
            backend_tls_policies: backend_tls_policies_cache,
            edgion_config_data: {
                let ed_cache = ClientCache::new(client_id.clone(), client_name.clone());
                let ed_handler = crate::plugins::runtime::edgion_config_data::create_edgion_config_data_handler();
                ed_cache.set_conf_processor(ed_handler);
                ed_cache
            },
            edgion_backends: {
                let cache = ClientCache::new(client_id.clone(), client_name.clone());
                cache.set_conf_processor(crate::ai::create_edgion_backend_handler());
                cache
            },
            edgion_acme: edgion_acme_cache,
            edgion_backend_traffic_policy: edgion_backend_traffic_policy_cache,
            required_cache_names: RwLock::new(None),
        }
    }

    /// Set required resource kinds for readiness checks.
    ///
    /// Only kinds that are both known and cached on Gateway are retained.
    /// If no valid cache kind is resolved, readiness falls back to checking all caches.
    pub fn set_required_kinds_for_readiness(&self, kind_names: &[String]) {
        let mut required = HashSet::new();

        for kind_name in kind_names {
            let Some(kind) = ResourceKind::from_kind_name(kind_name) else {
                continue;
            };
            // Skip kinds intentionally not cached on Gateway.
            if self.get_dyn_cache(kind).is_none() {
                continue;
            }
            if let Some(info) = get_resource_info(kind) {
                required.insert(info.cache_field_name.to_string());
            }
        }

        let mut guard = self.required_cache_names.write();
        if required.is_empty() {
            *guard = None;
            tracing::warn!(
                component = "config_client",
                supported_kinds = ?kind_names,
                "No valid cache kinds resolved for readiness; fallback to all caches"
            );
            return;
        }
        *guard = Some(required);
    }

    /// Set gRPC client for all caches that have a DynClientCache entry.
    ///
    /// Iterates over all ResourceKind variants via get_dyn_cache (exhaustive match),
    /// so adding a new kind without wiring it will cause a compile error.
    pub async fn set_all_grpc_clients(
        &self,
        client: edgion_common::conf_sync::proto::config_sync_client::ConfigSyncClient<tonic::transport::Channel>,
    ) {
        use edgion_resources::resource_defs::all_resource_kind_names;

        for cache_field in all_resource_kind_names() {
            if let Some(kind) = edgion_resources::resource_defs::resource_kind_from_cache_field(cache_field) {
                if let Some(cache) = self.get_dyn_cache(kind) {
                    cache.set_grpc_client_dyn(client.clone()).await;
                }
            }
        }
    }

    /// Get the current server_id from Controller
    pub fn current_server_id(&self) -> String {
        self.current_server_id.read().clone()
    }

    /// Update the current server_id (called by cache when receiving list/watch responses)
    pub fn set_current_server_id(&self, server_id: String) {
        let mut current = self.current_server_id.write();
        if *current != server_id {
            tracing::debug!(
                component = "config_client",
                old_server_id = %*current,
                new_server_id = %server_id,
                "Server ID updated"
            );
            *current = server_id;
        }
    }

    /// Typed accessor for the `EdgionTls` cache, retained for typed test access.
    #[cfg(test)]
    pub fn edgion_tls(&self) -> &ClientCache<EdgionTls> {
        &self.edgion_tls
    }

    /// Typed accessor retained for Admin redaction tests.
    #[cfg(test)]
    pub fn edgion_backends(&self) -> &ClientCache<EdgionBackend> {
        &self.edgion_backends
    }

    /// Get the DynClientCache for a given ResourceKind.
    ///
    /// Uses exhaustive match on ResourceKind to ensure compile-time completeness.
    /// When a new ResourceKind variant is added, the compiler will force you to
    /// handle it here — eliminating the "UnknownKind" dead-wait bug.
    ///
    /// Returns None for kinds that are intentionally not cached on Gateway
    /// (Secret, ConfigMap, ReferenceGrant, Unspecified).
    pub fn get_dyn_cache(&self, kind: ResourceKind) -> Option<&dyn DynClientCache> {
        match kind {
            ResourceKind::GatewayClass => Some(&self.gateway_classes),
            ResourceKind::Gateway => Some(&self.gateways),
            ResourceKind::EdgionGatewayConfig => Some(&self.edgion_gateway_configs),
            ResourceKind::HTTPRoute => Some(&self.routes),
            ResourceKind::GRPCRoute => Some(&self.grpc_routes),
            ResourceKind::TCPRoute => Some(&self.tcp_routes),
            ResourceKind::UDPRoute => Some(&self.udp_routes),
            ResourceKind::TLSRoute => Some(&self.tls_routes),
            ResourceKind::LinkSys => Some(&self.link_sys),
            ResourceKind::Service => Some(&self.services),
            ResourceKind::EndpointSlice => Some(&self.endpoint_slices),
            ResourceKind::EdgionTls => Some(&self.edgion_tls),
            ResourceKind::EdgionAcme => Some(&self.edgion_acme),
            ResourceKind::EdgionPlugins => Some(&self.edgion_plugins),
            ResourceKind::EdgionStreamPlugins => Some(&self.edgion_stream_plugins),
            ResourceKind::BackendTLSPolicy => Some(&self.backend_tls_policies),
            ResourceKind::EdgionConfigData => Some(&self.edgion_config_data),
            ResourceKind::EdgionBackend => Some(&self.edgion_backends),
            // These are intentionally not cached on Gateway side
            ResourceKind::Secret => None,
            ResourceKind::ConfigMap => None,
            ResourceKind::ReferenceGrant => None,
            ResourceKind::Unspecified => None,
            ResourceKind::EdgionBackendTrafficPolicy => Some(&self.edgion_backend_traffic_policy),
        }
    }

    /// Get cache ready status by name (delegates to get_dyn_cache)
    fn get_cache_status(&self, name: &str) -> Option<bool> {
        let kind = edgion_resources::resource_defs::resource_kind_from_cache_field(name)?;
        self.get_dyn_cache(kind).map(|c| c.is_ready())
    }

    /// Returns `(name, ready)` pairs for all caches, filtered by the required-kinds whitelist.
    fn all_caches_status(&self) -> Vec<(&'static str, bool)> {
        let required = self.required_cache_names.read().clone();
        all_resource_type_names()
            .into_iter()
            .filter(|name| required.as_ref().map(|set| set.contains(*name)).unwrap_or(true))
            .filter_map(|name| self.get_cache_status(name).map(|ready| (name, ready)))
            .collect()
    }

    /// Check if all caches are ready
    /// Returns Ok(()) if all caches are ready, Err with waiting message otherwise
    pub fn is_ready(&self) -> Result<(), String> {
        let not_ready: Vec<&str> = self
            .all_caches_status()
            .into_iter()
            .filter_map(|(name, ready)| if !ready { Some(name) } else { None })
            .collect();

        if not_ready.is_empty() {
            Ok(())
        } else {
            Err(format!("wait [{}] ready", not_ready.join(", ")))
        }
    }

    /// List HTTP routes
    pub fn list_routes(&self) -> ListData<HTTPRoute> {
        self.routes.list_owned()
    }

    /// List gRPC routes
    pub fn list_grpc_routes(&self) -> ListData<GRPCRoute> {
        self.grpc_routes.list_owned()
    }

    /// List TCP routes
    pub fn list_tcp_routes(&self) -> ListData<TCPRoute> {
        self.tcp_routes.list_owned()
    }

    /// List UDP routes
    pub fn list_udp_routes(&self) -> ListData<UDPRoute> {
        self.udp_routes.list_owned()
    }

    /// List TLS routes
    pub fn list_tls_routes(&self) -> ListData<TLSRoute> {
        self.tls_routes.list_owned()
    }

    /// List GatewayClasses
    pub fn list_gateway_classes(&self) -> ListData<GatewayClass> {
        self.gateway_classes.list_owned()
    }

    /// List Gateways
    pub fn list_gateways(&self) -> ListData<Gateway> {
        let mut gateways = self.gateways.list_owned();
        if let Some(scope) = self.gateway_scope.as_deref() {
            gateways.data.retain(|gateway| {
                gateway.key_name() == scope
                    && self
                        .gateway_uid
                        .as_deref()
                        .is_none_or(|uid| gateway.metadata.uid.as_deref() == Some(uid))
            });
        } else {
            let dedicated = self.shared_scope.snapshot();
            gateways.data.retain(|gateway| !dedicated.contains(&gateway.key_name()));
        }
        gateways
    }

    /// List EdgionGatewayConfigs
    pub fn list_edgion_gateway_configs(&self) -> ListData<EdgionGatewayConfig> {
        self.edgion_gateway_configs.list_owned()
    }

    /// Get a GatewayClass by name
    pub fn get_gateway_class(&self, name: &str) -> Option<GatewayClass> {
        // GatewayClass is cluster-scoped (no namespace)
        self.gateway_classes.get(name)
    }

    /// Get an EdgionGatewayConfig by name
    pub fn get_edgion_gateway_config(&self, name: &str) -> Option<EdgionGatewayConfig> {
        // EdgionGatewayConfig is cluster-scoped (no namespace)
        self.edgion_gateway_configs.get(name)
    }

    /// Per-kind entry counts for every ClientCache held on the Gateway side.
    /// Used by store-stats leak detection.
    ///
    /// Iterates `ALL_RESOURCE_INFOS` and dispatches through `get_dyn_cache()`,
    /// which is an exhaustive `ResourceKind` match. Kinds not cached on Gateway
    /// (Secret, ConfigMap, ReferenceGrant) return `None` and are skipped. Adding a
    /// new `ResourceKind` automatically appears here as soon as it is wired into
    /// `get_dyn_cache()` — no separate string table to keep in sync.
    pub fn client_cache_stats(&self) -> std::collections::BTreeMap<String, usize> {
        use edgion_resources::resource_defs::ALL_RESOURCE_INFOS;

        let mut out = std::collections::BTreeMap::new();
        for info in ALL_RESOURCE_INFOS {
            if let Some(cache) = self.get_dyn_cache(info.kind) {
                out.insert(cache.kind_name_dyn().to_string(), cache.len_dyn());
            }
        }
        out
    }

    /// Per-kind sample keys (capped at `max`).
    ///
    /// Looks up the cache via `get_dyn_cache()` (exhaustive `ResourceKind` match) and
    /// matches `kind` against `kind_name_dyn()` — so it stays in lockstep with
    /// `client_cache_stats`. Returns an empty Vec for any unknown kind or for kinds
    /// not cached on Gateway (Secret, ConfigMap, ReferenceGrant).
    /// Test-only: exercised by the stats/keys lockstep cross-check below; no
    /// production caller today.
    #[cfg(test)]
    pub fn client_cache_keys(&self, kind: &str, max: usize) -> Vec<String> {
        use edgion_resources::resource_defs::ALL_RESOURCE_INFOS;

        for info in ALL_RESOURCE_INFOS {
            let Some(cache) = self.get_dyn_cache(info.kind) else {
                continue;
            };
            if cache.kind_name_dyn() == kind {
                return cache.keys_dyn().into_iter().take(max).collect();
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_sync/conf_client/config_client_test.rs"]
mod tests;
