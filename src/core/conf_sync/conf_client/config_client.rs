use crate::core::backends::{create_endpoint_handler, create_ep_slice_handler, create_service_handler};
use crate::core::conf_sync::cache_client::ClientCache;
use crate::core::conf_sync::traits::{CacheEventDispatch, ConfigClientEventDispatcher, ResourceChange};
use crate::core::conf_sync::types::ListData;
use crate::core::routes::create_route_manager_handler;
use crate::core::utils::format_resource_info;
use crate::types::prelude_resources::*;
use crate::types::{all_resource_type_names, GatewayBaseConf, ResourceMeta};
use anyhow::Result;
use k8s_openapi::api::core::v1::{Endpoints, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;
use std::sync::RwLock;

pub struct ConfigClient {
    pub base_conf: RwLock<Option<GatewayBaseConf>>,
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
    reference_grants: ClientCache<ReferenceGrant>,
    backend_tls_policies: ClientCache<BackendTLSPolicy>,
    plugin_metadata: ClientCache<PluginMetaData>,
    // secrets: ClientCache<Secret>,  // Secret now follows related resources
}

impl ConfigClient {
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

        // Register ReferenceGrantStore as the handler for ReferenceGrant resources
        let reference_grants_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let reference_grant_handler = crate::core::ref_grant::create_reference_grant_handler();
        reference_grants_cache.set_conf_processor(reference_grant_handler);

        // Register BackendTLSPolicyStore as the handler for BackendTLSPolicy resources
        let backend_tls_policies_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let backend_tls_policy_handler = crate::core::backends::create_backend_tls_policy_handler();
        backend_tls_policies_cache.set_conf_processor(backend_tls_policy_handler);

        // Register handlers for base conf resources
        let gateway_classes_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let gateway_class_handler = crate::core::gateway::gateway_class::create_gateway_class_handler();
        gateway_classes_cache.set_conf_processor(gateway_class_handler);

        let gateways_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let gateway_handler = crate::core::gateway::gateway_handler::create_gateway_handler();
        gateways_cache.set_conf_processor(gateway_handler);

        let edgion_gateway_configs_cache = ClientCache::new(client_id.clone(), client_name.clone());
        let edgion_gateway_config_handler =
            crate::core::gateway::edgion_gateway_config::create_edgion_gateway_config_handler();
        edgion_gateway_configs_cache.set_conf_processor(edgion_gateway_config_handler);

        Self {
            base_conf: RwLock::new(None),
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
            reference_grants: reference_grants_cache,
            backend_tls_policies: backend_tls_policies_cache,
            plugin_metadata: ClientCache::new(client_id, client_name),
            // secrets: ClientCache::new(client_id, client_name),
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

    /// Get edgion_plugins cache for direct access
    pub fn edgion_plugins(&self) -> &ClientCache<EdgionPlugins> {
        &self.edgion_plugins
    }

    /// Get edgion_stream_plugins cache for direct access
    pub fn edgion_stream_plugins(&self) -> &ClientCache<EdgionStreamPlugins> {
        &self.edgion_stream_plugins
    }

    /// Get reference_grants cache for direct access
    pub fn reference_grants(&self) -> &ClientCache<ReferenceGrant> {
        &self.reference_grants
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
            "reference_grants" => Some(self.reference_grants.is_ready()),
            "backend_tls_policies" => Some(self.backend_tls_policies.is_ready()),
            "plugin_metadata" => Some(self.plugin_metadata.is_ready()),
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
        let (data_json, resource_version) = match kind {
            ResourceKind::Unspecified => {
                return Err("Resource kind unspecified".to_string());
            }
            ResourceKind::GatewayClass => {
                let list_data = self.gateway_classes.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize GatewayClass data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionGatewayConfig => {
                let list_data = self.edgion_gateway_configs.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionGatewayConfig data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Gateway => {
                let list_data = self.gateways.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Gateway data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::HTTPRoute => {
                let list_data = self.routes.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize HTTPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::GRPCRoute => {
                let list_data = self.grpc_routes.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize GRPCRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::TCPRoute => {
                let list_data = self.tcp_routes.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize TCPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::UDPRoute => {
                let list_data = self.udp_routes.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize UDPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::TLSRoute => {
                let list_data = self.tls_routes.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize TLSRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::LinkSys => {
                let list_data = self.link_sys.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize LinkSys data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Service => {
                let list_data = self.services.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Service data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EndpointSlice => {
                let list_data = self.endpoint_slices.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EndpointSlice data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Endpoint => {
                let list_data = self.endpoints.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Endpoints data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionTls => {
                let list_data = self.edgion_tls.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionTls data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionPlugins => {
                let list_data = self.edgion_plugins.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionPlugins data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionStreamPlugins => {
                let list_data = self.edgion_stream_plugins.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionStreamPlugins data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::ReferenceGrant => {
                let list_data = self.reference_grants.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize ReferenceGrant data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::BackendTLSPolicy => {
                let list_data = self.backend_tls_policies.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize BackendTLSPolicy data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::PluginMetaData => {
                let list_data = self.plugin_metadata.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize PluginMetaData data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Secret => {
                // Secret now follows related resources, not stored separately
                return Err("Secret resources are not stored in ConfigClient".to_string());
            } // ResourceKind::Secret => {
              //     let list_data = self.secrets.list();
              //     let json = serde_json::to_string(&list_data.data)
              //         .map_err(|e| format!("Failed to serialize Secret data: {}", e))?;
              //     (json, list_data.resource_version)
              // }
        };

        Ok(ListDataSimple {
            data: data_json,
            resource_version,
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

    /// Print all configuration
    /// Format is identical to ConfigCenter::print_config
    pub fn print_config(&self) {
        println!("=== ConfigHub Config ===");

        // GatewayClass resources from cache
        let gateway_classes = self.list_gateway_classes();
        if !gateway_classes.data.is_empty() {
            println!(
                "GatewayClasses (count: {}, version: {}):",
                gateway_classes.data.len(),
                gateway_classes.resource_version
            );
            for (idx, gc) in gateway_classes.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(gc));
            }
        } else {
            println!("GatewayClasses: not found");
        }

        // EdgionGatewayConfig resources from cache
        let edgion_gateway_configs = self.list_edgion_gateway_configs();
        if !edgion_gateway_configs.data.is_empty() {
            println!(
                "EdgionGatewayConfigs (count: {}, version: {}):",
                edgion_gateway_configs.data.len(),
                edgion_gateway_configs.resource_version
            );
            for (idx, egwc) in edgion_gateway_configs.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(egwc));
            }
        } else {
            println!("EdgionGatewayConfigs: not found");
        }

        // Gateway resources from cache
        let gateways = self.list_gateways();
        if !gateways.data.is_empty() {
            println!(
                "Gateways (count: {}, version: {}):",
                gateways.data.len(),
                gateways.resource_version
            );
            for (idx, gw) in gateways.data.iter().enumerate() {
                println!("  [{}] {}", idx, format_resource_info(gw));
            }
        } else {
            println!("Gateways: not found");
        }

        // HTTP Routes
        let list_data = self.list_routes();
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

        // // Secrets
        // let list_data = self.list_secrets();
        // println!(
        //     "Secrets (count: {}, version: {}):",
        //     list_data.data.len(),
        //     list_data.resource_version
        // );
        // for (idx, secret) in list_data.data.iter().enumerate() {
        //     println!("  [{}] {}", idx, format_resource_info(secret));
        // }

        println!("=== End ConfigHub Config ===\n");
    }
}

pub struct ListDataSimple {
    pub data: String,
    pub resource_version: u64,
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

        match resource_type {
            ResourceKind::Unspecified => {
                eprintln!(
                    "[HUB] apply_resource_change {:?}: Unspecified resource kind, skipping (data: {})",
                    change,
                    &data[..data.len().min(200)]
                );
            }
            ResourceKind::HTTPRoute => match serde_yaml::from_str::<HTTPRoute>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.routes, change, resource);
                }
                Err(e) => log_error("HTTPRoute", &e),
            },
            ResourceKind::GRPCRoute => match serde_yaml::from_str::<GRPCRoute>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.grpc_routes, change, resource);
                }
                Err(e) => log_error("GRPCRoute", &e),
            },
            ResourceKind::TCPRoute => match serde_yaml::from_str::<TCPRoute>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.tcp_routes, change, resource);
                }
                Err(e) => log_error("TCPRoute", &e),
            },
            ResourceKind::UDPRoute => match serde_yaml::from_str::<UDPRoute>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.udp_routes, change, resource);
                }
                Err(e) => log_error("UDPRoute", &e),
            },
            ResourceKind::TLSRoute => match serde_yaml::from_str::<TLSRoute>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.tls_routes, change, resource);
                }
                Err(e) => log_error("TLSRoute", &e),
            },
            ResourceKind::LinkSys => match serde_yaml::from_str::<LinkSys>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.link_sys, change, resource);
                }
                Err(e) => log_error("LinkSys", &e),
            },
            ResourceKind::Service => match serde_yaml::from_str::<Service>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.services, change, resource);
                }
                Err(e) => log_error("Service", &e),
            },
            ResourceKind::EndpointSlice => match serde_yaml::from_str::<EndpointSlice>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.endpoint_slices, change, resource);
                }
                Err(e) => log_error("EndpointSlice", &e),
            },
            ResourceKind::Endpoint => match serde_yaml::from_str::<Endpoints>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.endpoints, change, resource);
                }
                Err(e) => log_error("Endpoints", &e),
            },
            ResourceKind::EdgionTls => match serde_yaml::from_str::<EdgionTls>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.edgion_tls, change, resource);
                }
                Err(e) => log_error("EdgionTls", &e),
            },
            ResourceKind::EdgionPlugins => match serde_yaml::from_str::<EdgionPlugins>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.edgion_plugins, change, resource);
                }
                Err(e) => log_error("EdgionPlugins", &e),
            },
            ResourceKind::EdgionStreamPlugins => match serde_yaml::from_str::<EdgionStreamPlugins>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.edgion_stream_plugins, change, resource);
                }
                Err(e) => log_error("EdgionStreamPlugins", &e),
            },
            ResourceKind::ReferenceGrant => match serde_yaml::from_str::<ReferenceGrant>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.reference_grants, change, resource);
                }
                Err(e) => log_error("ReferenceGrant", &e),
            },
            ResourceKind::BackendTLSPolicy => match serde_yaml::from_str::<BackendTLSPolicy>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.backend_tls_policies, change, resource);
                }
                Err(e) => log_error("BackendTLSPolicy", &e),
            },
            ResourceKind::PluginMetaData => match serde_yaml::from_str::<PluginMetaData>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.plugin_metadata, change, resource);
                }
                Err(e) => log_error("PluginMetaData", &e),
            },
            // ResourceKind::Secret => match serde_yaml::from_str::<Secret>(&data) {
            //     Ok(resource) => {
            //         Self::apply_change_to_cache(&self.secrets, change, resource);
            //     }
            //     Err(e) => log_error("Secret", &e),
            // },
            ResourceKind::Secret => {
                tracing::warn!("skip resource change {:?} for Secret", change);
            }
            ResourceKind::GatewayClass => match serde_yaml::from_str::<GatewayClass>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.gateway_classes, change, resource);
                }
                Err(e) => log_error("GatewayClass", &e),
            },
            ResourceKind::Gateway => match serde_yaml::from_str::<Gateway>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.gateways, change, resource);
                }
                Err(e) => log_error("Gateway", &e),
            },
            ResourceKind::EdgionGatewayConfig => match serde_yaml::from_str::<EdgionGatewayConfig>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.edgion_gateway_configs, change, resource);
                }
                Err(e) => log_error("EdgionGatewayConfig", &e),
            },
        }
    }
}
