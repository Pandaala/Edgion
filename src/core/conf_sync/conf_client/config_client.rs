use crate::types::{GatewayBaseConf, ResourceMeta};
use crate::core::conf_sync::cache_client::ClientCache;
use crate::core::conf_sync::types::ListData;
use crate::core::conf_sync::traits::{CacheEventDispatch, ConfigClientEventDispatcher, ResourceChange};
use crate::core::utils::format_resource_info;
use crate::core::routes::create_route_manager_handler;
use crate::core::backends::{create_service_handler, create_ep_slice_handler};
use crate::types::prelude_resources::*;
use anyhow::Result;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;
use std::sync::RwLock;

pub struct ConfigClient {
    gateway_class_key: String,
    pub base_conf: RwLock<Option<GatewayBaseConf>>,
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
    plugin_metadata: ClientCache<PluginMetaData>,
    // secrets: ClientCache<Secret>,  // Secret now follows related resources
}

impl ConfigClient {
    pub fn new(gateway_class_key: String, client_id: String, client_name: String) -> Self {
        // Register RouteManager as the handler for HTTPRoute resources
        let routes_cache = ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone());
        let route_handler = create_route_manager_handler();
        routes_cache.set_conf_processor(route_handler);

        // Register ServiceStore as the handler for Service resources
        let services_cache = ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone());
        let service_handler = create_service_handler();
        services_cache.set_conf_processor(service_handler);

        // Register EpSliceHandler as the handler for EndpointSlice resources
        let endpoint_slices_cache = ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone());
        let ep_slice_handler = create_ep_slice_handler();
        endpoint_slices_cache.set_conf_processor(ep_slice_handler);

        // Register PluginStore as the handler for EdgionPlugins resources
        let plugins_cache = ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone());
        let plugin_handler = crate::core::plugins::create_plugin_handler();
        plugins_cache.set_conf_processor(plugin_handler);
        
        // Register TcpRouteManager as the handler for TCPRoute resources
        let tcp_routes_cache = ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone());
        let tcp_route_handler = crate::core::routes::tcp_routes::create_tcp_route_handler();
        tcp_routes_cache.set_conf_processor(tcp_route_handler);
        
        // Register UdpRouteManager as the handler for UDPRoute resources
        let udp_routes_cache = ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone());
        let udp_route_handler = crate::core::routes::udp_routes::create_udp_route_handler();
        udp_routes_cache.set_conf_processor(udp_route_handler);
        
        // Register GrpcRouteManager as the handler for GRPCRoute resources
        let grpc_routes_cache = ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone());
        let grpc_route_handler = crate::core::routes::grpc_routes::create_grpc_route_handler();
        grpc_routes_cache.set_conf_processor(grpc_route_handler);
        
        // Register TlsStore as the handler for EdgionTls resources
        let edgion_tls_cache = ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone());
        let tls_handler = crate::core::tls::create_tls_handler();
        edgion_tls_cache.set_conf_processor(tls_handler);
        
        Self {
            gateway_class_key: gateway_class_key.clone(),
            base_conf: RwLock::new(None),
            routes: routes_cache,
            grpc_routes: grpc_routes_cache,
            tcp_routes: tcp_routes_cache,
            udp_routes: udp_routes_cache,
            tls_routes: ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone()),
            link_sys: ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone()),
            services: services_cache,
            endpoint_slices: endpoint_slices_cache,
            edgion_tls: edgion_tls_cache,
            edgion_plugins: plugins_cache,
            plugin_metadata: ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone()),
            // secrets: ClientCache::new(gateway_class_key, client_id, client_name),
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

    /// Get edgion_tls cache for direct access
    pub fn edgion_tls(&self) -> &ClientCache<EdgionTls> {
        &self.edgion_tls
    }

    /// Get edgion_plugins cache for direct access
    pub fn edgion_plugins(&self) -> &ClientCache<EdgionPlugins> {
        &self.edgion_plugins
    }

    /// Get plugin_metadata cache for direct access
    pub fn plugin_metadata(&self) -> &ClientCache<PluginMetaData> {
        &self.plugin_metadata
    }

    // /// Get secrets cache for direct access
    // pub fn secrets(&self) -> &ClientCache<Secret> {
    //     &self.secrets
    // }

    pub fn get_gateway_class_key(&self) -> &String {
        &self.gateway_class_key
    }

    /// Check if all caches are ready
    /// Returns Ok(()) if all caches are ready, Err with waiting message otherwise
    pub fn is_ready(&self) -> Result<(), String> {
        let mut not_ready = Vec::new();
        
        if !self.routes.is_ready() {
            not_ready.push("routes");
        }
        if !self.grpc_routes.is_ready() {
            not_ready.push("grpc_routes");
        }
        if !self.tcp_routes.is_ready() {
            not_ready.push("tcp_routes");
        }
        if !self.udp_routes.is_ready() {
            not_ready.push("udp_routes");
        }
        if !self.tls_routes.is_ready() {
            not_ready.push("tls_routes");
        }
        if !self.link_sys.is_ready() {
            not_ready.push("link_sys");
        }
        if !self.services.is_ready() {
            not_ready.push("services");
        }
        if !self.endpoint_slices.is_ready() {
            not_ready.push("endpoint_slices");
        }
        if !self.edgion_tls.is_ready() {
            not_ready.push("edgion_tls");
        }
        if !self.edgion_plugins.is_ready() {
            not_ready.push("edgion_plugins");
        }
        if !self.plugin_metadata.is_ready() {
            not_ready.push("plugin_metadata");
        }
        // if !self.secrets.is_ready() {
        //     not_ready.push("secrets");
        // }
        
        if not_ready.is_empty() {
            Ok(())
        } else {
            Err(format!("wait [{}] ready", not_ready.join(", ")))
        }
    }

    /// Initialize base configuration with parsed objects
    pub fn init_base_conf(&self, new_base_conf: GatewayBaseConf) {
        let mut base_conf = self.base_conf.write().unwrap();
        *base_conf = Some(new_base_conf);
    }

    /// Get a copy of the current base configuration
    pub fn get_base_conf(&self) -> Option<GatewayBaseConf> {
        let base_conf = self.base_conf.read().unwrap();
        base_conf.clone()
    }

    fn apply_change_to_cache<T>(cache: &ClientCache<T>, change: ResourceChange, resource: T)
    where
        T: Clone + ResourceMeta + Resource + Send + 'static,
    {
        cache.apply_change(change, resource);
    }

    pub fn list(&self, key: &String, kind: &ResourceKind) -> Result<ListDataSimple, String> {
        if key != &self.gateway_class_key {
            return Err(format!(
                "Key mismatch: expected {}, got {}",
                self.gateway_class_key, key
            ));
        }

        let (data_json, resource_version) = match kind {
            ResourceKind::Unspecified => {
                return Err("Resource kind unspecified".to_string());
            }
            ResourceKind::GatewayClass => {
                let base_conf_guard = self.base_conf.read().unwrap();
                let data: Vec<GatewayClass> = if let Some(ref base_conf) = *base_conf_guard {
                    vec![base_conf.gateway_class().clone()]
                } else {
                    vec![]
                };
                let json = serde_json::to_string(&data)
                    .map_err(|e| format!("Failed to serialize GatewayClass data: {}", e))?;
                // Base conf resources don't have version tracking, use 0
                (json, 0)
            }
            ResourceKind::EdgionGatewayConfig => {
                let base_conf_guard = self.base_conf.read().unwrap();
                let data: Vec<EdgionGatewayConfig> = if let Some(ref base_conf) = *base_conf_guard {
                    vec![base_conf.edgion_gateway_config().clone()]
                } else {
                    vec![]
                };
                let json = serde_json::to_string(&data)
                    .map_err(|e| format!("Failed to serialize EdgionGatewayConfig data: {}", e))?;
                // Base conf resources don't have version tracking, use 0
                (json, 0)
            }
            ResourceKind::Gateway => {
                let base_conf_guard = self.base_conf.read().unwrap();
                let data = if let Some(ref base_conf) = *base_conf_guard {
                    base_conf.gateways().clone()
                } else {
                    vec![]
                };
                let json =
                    serde_json::to_string(&data).map_err(|e| format!("Failed to serialize Gateway data: {}", e))?;
                // Base conf resources don't have version tracking, use 0
                (json, 0)
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
            ResourceKind::PluginMetaData => {
                let list_data = self.plugin_metadata.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize PluginMetaData data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Secret => {
                // Secret now follows related resources, not stored separately
                return Err("Secret resources are not stored in ConfigClient".to_string());
            }
            // ResourceKind::Secret => {
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

    // /// List secrets
    // pub fn list_secrets(&self) -> ListData<Secret> {
    //     self.secrets.list_owned()
    // }

    /// Trigger update event for an endpoint slice by key
    pub fn trigger_endpoint_slice_update_event(&self, key: &str) {
        self.endpoint_slices.trigger_update_event_by_key(key);
    }

    /// Print all configuration for the gateway class key
    /// Format is identical to ConfigCenter::print_config
    pub fn print_config(&self) {
        let key = &self.gateway_class_key;
        println!("=== ConfigHub Config for GatewayClassKey: {} ===", key);

        // Base conf resources are stored in base_conf
        let base_conf_guard = self.base_conf.read().unwrap();
        if let Some(ref base_conf) = *base_conf_guard {
            println!("GatewayClass:");
            println!("  [0] {}", format_resource_info(base_conf.gateway_class()));

            println!("EdgionGatewayConfig:");
            println!("  [0] {}", format_resource_info(base_conf.edgion_gateway_config()));

            let gateways = base_conf.gateways();
            if !gateways.is_empty() {
                println!("Gateways (count: {}):", gateways.len());
                for (idx, gw) in gateways.iter().enumerate() {
                    println!("  [{}] {}", idx, format_resource_info(gw));
                }
            } else {
                println!("Gateways: not found");
            }
        } else {
            println!("Base configuration not initialized");
        }
        drop(base_conf_guard);

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
            ResourceKind::Secret | ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                tracing::warn!("skip resource change {:?}", change);
            }
        }
    }
}
