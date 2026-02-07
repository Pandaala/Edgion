use crate::core::backends::{create_endpoint_handler, create_ep_slice_handler, create_service_handler};
use crate::core::conf_sync::cache_client::ClientCache;
use crate::core::conf_sync::traits::{CacheEventDispatch, ConfigClientEventDispatcher, ResourceChange};
use crate::core::conf_sync::types::ListData;
use crate::core::routes::create_route_manager_handler;
use crate::types::prelude_resources::*;
use crate::types::{all_resource_type_names, GatewayBaseConf, ResourceMeta};
use anyhow::Result;
use k8s_openapi::api::core::v1::{Endpoints, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;
use std::sync::RwLock;

pub struct ConfigClient {
    pub base_conf: RwLock<Option<GatewayBaseConf>>,
    /// Current server_id from Controller (updated on each list/watch response)
    current_server_id: RwLock<String>,
    // Base conf resources now have dedicated caches
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
    endpoints: ClientCache<Endpoints>,
    edgion_tls: ClientCache<EdgionTls>,
    edgion_plugins: ClientCache<EdgionPlugins>,
    edgion_stream_plugins: ClientCache<EdgionStreamPlugins>,
    backend_tls_policies: ClientCache<BackendTLSPolicy>,
    plugin_metadata: ClientCache<PluginMetaData>,
    edgion_acme: ClientCache<EdgionAcme>,
    // secrets: ClientCache<Secret>,  // Secret now follows related resources
}

impl ConfigClient {
    /// Create a new ConfigClient
    ///
    /// # Arguments
    /// * `client_id` - Unique identifier for this client
    /// * `client_name` - Human-readable name for this client
    pub fn new(client_id: String, client_name: String) -> Self {
        // Register RouteManager as the handler for HTTPRoute resources
        let routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let route_handler = create_route_manager_handler();
        routes_cache.set_conf_processor(route_handler);

        // Register ServiceStore as the handler for Service resources
        let services_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let service_handler = create_service_handler();
        services_cache.set_conf_processor(service_handler);

        // Register EpSliceHandler as the handler for EndpointSlice resources
        let endpoint_slices_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let ep_slice_handler = create_ep_slice_handler();
        endpoint_slices_cache.set_conf_processor(ep_slice_handler);

        // Register EndpointHandler as the handler for Endpoints resources
        let endpoints_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let endpoint_handler = create_endpoint_handler();
        endpoints_cache.set_conf_processor(endpoint_handler);

        // Register PluginStore as the handler for EdgionPlugins resources
        let plugins_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let plugin_handler = crate::core::plugins::edgion_plugins::create_plugin_handler();
        plugins_cache.set_conf_processor(plugin_handler);

        // Register TcpRouteManager as the handler for TCPRoute resources
        let tcp_routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let tcp_route_handler = crate::core::routes::tcp_routes::create_tcp_route_handler();
        tcp_routes_cache.set_conf_processor(tcp_route_handler);

        // Register UdpRouteManager as the handler for UDPRoute resources
        let udp_routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let udp_route_handler = crate::core::routes::udp_routes::create_udp_route_handler();
        udp_routes_cache.set_conf_processor(udp_route_handler);

        // Register GrpcRouteManager as the handler for GRPCRoute resources
        let grpc_routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let grpc_route_handler = crate::core::routes::grpc_routes::create_grpc_route_handler();
        grpc_routes_cache.set_conf_processor(grpc_route_handler);

        // Register TlsRouteManager as the handler for TLSRoute resources
        let tls_routes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let tls_route_handler = crate::core::routes::tls_routes::create_tls_route_handler();
        tls_routes_cache.set_conf_processor(tls_route_handler);

        // Register TlsStore as the handler for EdgionTls resources
        let edgion_tls_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let tls_handler = crate::core::tls::create_tls_handler();
        edgion_tls_cache.set_conf_processor(tls_handler);

        // Register StreamPluginStore as the handler for EdgionStreamPlugins resources
        let stream_plugins_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let stream_plugin_handler = crate::core::plugins::edgion_stream_plugins::create_stream_plugin_handler();
        stream_plugins_cache.set_conf_processor(stream_plugin_handler);

        // Register BackendTLSPolicyStore as the handler for BackendTLSPolicy resources
        let backend_tls_policies_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let backend_tls_policy_handler = crate::core::backends::create_backend_tls_policy_handler();
        backend_tls_policies_cache.set_conf_processor(backend_tls_policy_handler);

        // Register handlers for base conf resources
        let gateway_classes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let gateway_class_handler = crate::core::gateway::gateway_class::create_gateway_class_handler();
        gateway_classes_cache.set_conf_processor(gateway_class_handler);

        let gateways_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let gateway_handler = crate::core::gateway::gateway::create_gateway_handler();
        gateways_cache.set_conf_processor(gateway_handler);

        let edgion_gateway_configs_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let edgion_gateway_config_handler =
            crate::core::gateway::edgion_gateway_config::create_edgion_gateway_config_handler();
        edgion_gateway_configs_cache.set_conf_processor(edgion_gateway_config_handler);

        // Register AcmeChallengeStore as the handler for EdgionAcme resources
        let edgion_acme_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let acme_handler = crate::core::services::acme::create_acme_handler();
        edgion_acme_cache.set_conf_processor(acme_handler);

        Self {
            base_conf: RwLock::new(None),
            current_server_id: RwLock::new(String::new()),
            // Base conf caches with handlers registered
            gateway_classes: gateway_classes_cache,
            gateways: gateways_cache,
            edgion_gateway_configs: edgion_gateway_configs_cache,
            routes: routes_cache,
            grpc_routes: grpc_routes_cache,
            tcp_routes: tcp_routes_cache,
            udp_routes: udp_routes_cache,
            tls_routes: tls_routes_cache,
            link_sys: ClientCache::new(client_id.clone(), client_name.clone()),
            services: services_cache,
            endpoint_slices: endpoint_slices_cache,
            endpoints: endpoints_cache,
            edgion_tls: edgion_tls_cache,
            edgion_plugins: plugins_cache,
            edgion_stream_plugins: stream_plugins_cache,
            backend_tls_policies: backend_tls_policies_cache,
            plugin_metadata: ClientCache::new(client_id.clone(), client_name.clone()),
            edgion_acme: edgion_acme_cache,
        }
    }

    /// Get the current server_id from Controller
    pub fn current_server_id(&self) -> String {
        self.current_server_id.read().unwrap().clone()
    }

    /// Update the current server_id (called by cache when receiving list/watch responses)
    pub fn set_current_server_id(&self, server_id: String) {
        let mut current = self.current_server_id.write().unwrap();
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

    /// Get routes cache for direct access
    pub fn routes(&self) -> &ClientCache<HTTPRoute> {
        &self.routes
    }

    /// Get grpc_routes cache for direct access
    pub fn grpc_routes(&self) -> &ClientCache<GRPCRoute> {
        &self.grpc_routes
    }

    /// Get tcp_routes cache for direct access
    pub fn tcp_routes(&self) -> &ClientCache<TCPRoute> {
        &self.tcp_routes
    }

    /// Get udp_routes cache for direct access
    pub fn udp_routes(&self) -> &ClientCache<UDPRoute> {
        &self.udp_routes
    }

    /// Get tls_routes cache for direct access
    pub fn tls_routes(&self) -> &ClientCache<TLSRoute> {
        &self.tls_routes
    }

    /// Get link_sys cache for direct access
    pub fn link_sys(&self) -> &ClientCache<LinkSys> {
        &self.link_sys
    }

    /// Get services cache for direct access
    pub fn services(&self) -> &ClientCache<Service> {
        &self.services
    }

    /// Get endpoint_slices cache for direct access
    pub fn endpoint_slices(&self) -> &ClientCache<EndpointSlice> {
        &self.endpoint_slices
    }

    /// Get endpoints cache for direct access
    pub fn endpoints(&self) -> &ClientCache<Endpoints> {
        &self.endpoints
    }

    /// Get edgion_tls cache for direct access
    pub fn edgion_tls(&self) -> &ClientCache<EdgionTls> {
        &self.edgion_tls
    }

    /// Get edgion_acme cache for direct access
    pub fn edgion_acme(&self) -> &ClientCache<EdgionAcme> {
        &self.edgion_acme
    }

    /// Get edgion_plugins cache for direct access
    pub fn edgion_plugins(&self) -> &ClientCache<EdgionPlugins> {
        &self.edgion_plugins
    }

    /// Get edgion_stream_plugins cache for direct access
    pub fn edgion_stream_plugins(&self) -> &ClientCache<EdgionStreamPlugins> {
        &self.edgion_stream_plugins
    }

    /// Get backend_tls_policies cache for direct access
    pub fn backend_tls_policies(&self) -> &ClientCache<BackendTLSPolicy> {
        &self.backend_tls_policies
    }

    /// Get plugin_metadata cache for direct access
    pub fn plugin_metadata(&self) -> &ClientCache<PluginMetaData> {
        &self.plugin_metadata
    }

    /// Get gateway_classes cache for direct access
    pub fn gateway_classes(&self) -> &ClientCache<GatewayClass> {
        &self.gateway_classes
    }

    /// Get gateways cache for direct access
    pub fn gateways(&self) -> &ClientCache<Gateway> {
        &self.gateways
    }

    /// Get edgion_gateway_configs cache for direct access
    pub fn edgion_gateway_configs(&self) -> &ClientCache<EdgionGatewayConfig> {
        &self.edgion_gateway_configs
    }

    // /// Get secrets cache for direct access
    // pub fn secrets(&self) -> &ClientCache<Secret> {
    //     &self.secrets
    // }

    /// Get cache ready status by name
    /// Returns None if the cache name is not recognized
    fn get_cache_status(&self, name: &str) -> Option<bool> {
        match name {
            "gateway_classes" => Some(self.gateway_classes.is_ready()),
            "gateways" => Some(self.gateways.is_ready()),
            "edgion_gateway_configs" => Some(self.edgion_gateway_configs.is_ready()),
            "routes" => Some(self.routes.is_ready()),
            "grpc_routes" => Some(self.grpc_routes.is_ready()),
            "tcp_routes" => Some(self.tcp_routes.is_ready()),
            "udp_routes" => Some(self.udp_routes.is_ready()),
            "tls_routes" => Some(self.tls_routes.is_ready()),
            "link_sys" => Some(self.link_sys.is_ready()),
            "services" => Some(self.services.is_ready()),
            "endpoint_slices" => Some(self.endpoint_slices.is_ready()),
            "endpoints" => Some(self.endpoints.is_ready()),
            "edgion_tls" => Some(self.edgion_tls.is_ready()),
            "edgion_plugins" => Some(self.edgion_plugins.is_ready()),
            "edgion_stream_plugins" => Some(self.edgion_stream_plugins.is_ready()),
            "backend_tls_policies" => Some(self.backend_tls_policies.is_ready()),
            "plugin_metadata" => Some(self.plugin_metadata.is_ready()),
            "edgion_acme" => Some(self.edgion_acme.is_ready()),
            // "secrets" => Some(self.secrets.is_ready()),  // Secret follows related resources
            _ => None,
        }
    }

    /// Get all caches status based on global resource registry
    /// Returns a list of tuples: (cache_name, is_ready)
    fn all_caches_status(&self) -> Vec<(&'static str, bool)> {
        all_resource_type_names()
            .into_iter()
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

    /// Initialize base configuration with parsed objects
    fn apply_change_to_cache<T>(cache: &ClientCache<T>, change: ResourceChange, resource: T)
    where
        T: Clone + ResourceMeta + Resource + Send + 'static,
    {
        cache.apply_change(change, resource);
    }

    pub fn list(&self, _key: &String, kind: &ResourceKind) -> Result<ListDataSimple, String> {
        let (data_json, sync_version) = match kind {
            ResourceKind::Unspecified => return Err("Resource kind unspecified".to_string()),
            ResourceKind::GatewayClass => self.gateway_classes.list().to_json("GatewayClass")?,
            ResourceKind::EdgionGatewayConfig => self.edgion_gateway_configs.list().to_json("EdgionGatewayConfig")?,
            ResourceKind::Gateway => self.gateways.list().to_json("Gateway")?,
            ResourceKind::HTTPRoute => self.routes.list().to_json("HTTPRoute")?,
            ResourceKind::GRPCRoute => self.grpc_routes.list().to_json("GRPCRoute")?,
            ResourceKind::TCPRoute => self.tcp_routes.list().to_json("TCPRoute")?,
            ResourceKind::UDPRoute => self.udp_routes.list().to_json("UDPRoute")?,
            ResourceKind::TLSRoute => self.tls_routes.list().to_json("TLSRoute")?,
            ResourceKind::LinkSys => self.link_sys.list().to_json("LinkSys")?,
            ResourceKind::Service => self.services.list().to_json("Service")?,
            ResourceKind::EndpointSlice => self.endpoint_slices.list().to_json("EndpointSlice")?,
            ResourceKind::Endpoint => self.endpoints.list().to_json("Endpoints")?,
            ResourceKind::EdgionTls => self.edgion_tls.list().to_json("EdgionTls")?,
            ResourceKind::EdgionPlugins => self.edgion_plugins.list().to_json("EdgionPlugins")?,
            ResourceKind::EdgionStreamPlugins => self.edgion_stream_plugins.list().to_json("EdgionStreamPlugins")?,
            ResourceKind::ReferenceGrant => {
                return Err("ReferenceGrant resources are not synced to Gateway".to_string())
            }
            ResourceKind::BackendTLSPolicy => self.backend_tls_policies.list().to_json("BackendTLSPolicy")?,
            ResourceKind::PluginMetaData => self.plugin_metadata.list().to_json("PluginMetaData")?,
            ResourceKind::Secret => return Err("Secret resources are not stored in ConfigClient".to_string()),
            ResourceKind::EdgionAcme => self.edgion_acme.list().to_json("EdgionAcme")?,
        };

        Ok(ListDataSimple {
            data: data_json,
            sync_version,
        })
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

    /// List LinkSys
    pub fn list_link_sys(&self) -> ListData<LinkSys> {
        self.link_sys.list_owned()
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

    /// List plugin metadata
    pub fn list_plugin_metadata(&self) -> ListData<PluginMetaData> {
        self.plugin_metadata.list_owned()
    }

    /// List GatewayClasses
    pub fn list_gateway_classes(&self) -> ListData<GatewayClass> {
        self.gateway_classes.list_owned()
    }

    /// List Gateways
    pub fn list_gateways(&self) -> ListData<Gateway> {
        self.gateways.list_owned()
    }

    /// List EdgionGatewayConfigs
    pub fn list_edgion_gateway_configs(&self) -> ListData<EdgionGatewayConfig> {
        self.edgion_gateway_configs.list_owned()
    }

    /// Get a Gateway by namespace and name
    pub fn get_gateway(&self, namespace: &str, name: &str) -> Option<Gateway> {
        let key = format!("{}/{}", namespace, name);
        self.gateways.get(&key)
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

    // /// List secrets
    // pub fn list_secrets(&self) -> ListData<Secret> {
    //     self.secrets.list_owned()
    // }

    /// Trigger update event for an endpoint slice by key
    pub fn trigger_endpoint_slice_update_event(&self, key: &str) {
        self.endpoint_slices.trigger_update_event_by_key(key);
    }
}

pub struct ListDataSimple {
    pub data: String,
    pub sync_version: u64,
}

impl ConfigClientEventDispatcher for ConfigClient {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        _resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            eprintln!(
                "[HUB] apply_resource_change {:?}: Failed to determine resource type from data: {}",
                change,
                &data[..data.len().min(200)]
            );
            return;
        };

        let log_error = |kind: &str, err: &serde_yaml::Error| {
            eprintln!(
                "[HUB] apply_resource_change {:?}: Failed to parse {}: {} (data: {})",
                change,
                kind,
                err,
                &data[..data.len().min(200)]
            );
        };

        // Helper macro to reduce repetitive parse-and-apply code
        macro_rules! apply_change {
            ($type:ty, $field:ident, $name:literal) => {
                match serde_yaml::from_str::<$type>(&data) {
                    Ok(resource) => Self::apply_change_to_cache(&self.$field, change, resource),
                    Err(e) => log_error($name, &e),
                }
            };
        }

        match resource_type {
            ResourceKind::Unspecified => {
                eprintln!(
                    "[HUB] apply_resource_change {:?}: Unspecified resource kind, skipping (data: {})",
                    change,
                    &data[..data.len().min(200)]
                );
            }
            ResourceKind::GatewayClass => apply_change!(GatewayClass, gateway_classes, "GatewayClass"),
            ResourceKind::EdgionGatewayConfig => {
                apply_change!(EdgionGatewayConfig, edgion_gateway_configs, "EdgionGatewayConfig")
            }
            ResourceKind::Gateway => apply_change!(Gateway, gateways, "Gateway"),
            ResourceKind::HTTPRoute => apply_change!(HTTPRoute, routes, "HTTPRoute"),
            ResourceKind::GRPCRoute => apply_change!(GRPCRoute, grpc_routes, "GRPCRoute"),
            ResourceKind::TCPRoute => apply_change!(TCPRoute, tcp_routes, "TCPRoute"),
            ResourceKind::UDPRoute => apply_change!(UDPRoute, udp_routes, "UDPRoute"),
            ResourceKind::TLSRoute => apply_change!(TLSRoute, tls_routes, "TLSRoute"),
            ResourceKind::LinkSys => apply_change!(LinkSys, link_sys, "LinkSys"),
            ResourceKind::PluginMetaData => apply_change!(PluginMetaData, plugin_metadata, "PluginMetaData"),
            ResourceKind::Service => apply_change!(Service, services, "Service"),
            ResourceKind::EndpointSlice => apply_change!(EndpointSlice, endpoint_slices, "EndpointSlice"),
            ResourceKind::Endpoint => apply_change!(Endpoints, endpoints, "Endpoints"),
            ResourceKind::EdgionTls => apply_change!(EdgionTls, edgion_tls, "EdgionTls"),
            ResourceKind::EdgionPlugins => apply_change!(EdgionPlugins, edgion_plugins, "EdgionPlugins"),
            ResourceKind::EdgionStreamPlugins => {
                apply_change!(EdgionStreamPlugins, edgion_stream_plugins, "EdgionStreamPlugins")
            }
            ResourceKind::ReferenceGrant => {
                tracing::debug!(
                    "skip resource change {:?} for ReferenceGrant (not synced to Gateway)",
                    change
                )
            }
            ResourceKind::BackendTLSPolicy => apply_change!(BackendTLSPolicy, backend_tls_policies, "BackendTLSPolicy"),
            ResourceKind::Secret => tracing::warn!("skip resource change {:?} for Secret", change),
            ResourceKind::EdgionAcme => apply_change!(EdgionAcme, edgion_acme, "EdgionAcme"),
        }
    }
}
