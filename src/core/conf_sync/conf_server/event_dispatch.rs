use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::{ConfigServerEventDispatcher, CacheEventDispatch, ServerCache, ResourceMeta};
use crate::types::prelude_resources::*;
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;

impl ConfigServer {
    fn execute_change_on_cache<T>(change: ResourceChange, cache: &ServerCache<T>, resource: T)
    where
        T: Clone + Send + Sync + 'static + ResourceMeta + Resource,
    {
        cache.apply_change(change, resource);
    }

    fn apply_resource_change_with_resource_type(
        &self,
        change: ResourceChange,
        resource_type: ResourceKind,
        data: String,
    ) {
        match resource_type {
            ResourceKind::Unspecified => {
                eprintln!(
                    "[ConfigServer] apply_resource_change_with_resource_type {:?}: Unspecified resource kind, skipping",
                    change
                );
                return;
            }
            // Base conf resources are handled by apply_base_conf, not here
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                // This should never be reached due to the early return above,
                // but included for match_engine exhaustiveness
                unreachable!("Base conf resources should have been handled earlier")
            }
            ResourceKind::GRPCRoute => {
                if let Ok(resource) = serde_yaml::from_str::<GRPCRoute>(&data) {
                    // Check if GRPCRoute references a gateway that exists in base_conf
                    let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
                        if let Some(first_ref) = parent_refs.first() {
                            // Get gateway namespace (if not specified, use GRPCRoute's namespace)
                            let gateway_namespace = first_ref
                                .namespace
                                .as_ref()
                                .or_else(|| resource.metadata.namespace.as_ref());
                            let gateway_name = Some(&first_ref.name);

                            let base_conf_guard = self.base_conf.read().unwrap();
                            base_conf_guard.has_gateway(gateway_namespace, gateway_name)
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if !gateway_exists {
                        let (gateway_info, message) = if let Some(parent_refs) = &resource.spec.parent_refs {
                            if let Some(first_ref) = parent_refs.first() {
                                let info = format!(
                                    "namespace={:?}, name={}",
                                    first_ref
                                        .namespace
                                        .as_ref()
                                        .or_else(|| resource.metadata.namespace.as_ref()),
                                    first_ref.name
                                );
                                (info, "GRPCRoute references a Gateway that does not exist in base_conf, skipping")
                            } else {
                                (
                                    "no parent_refs".to_string(),
                                    "GRPCRoute has empty parent_refs, skipping",
                                )
                            }
                        } else {
                            ("no parent_refs".to_string(), "GRPCRoute has no parent_refs, skipping")
                        };

                        tracing::warn!(
                            component = "config_server",
                            change = ?change,
                            kind = "GRPCRoute",
                            route_name = ?resource.metadata.name,
                            route_namespace = ?resource.metadata.namespace,
                            gateway = gateway_info,
                            "{}",
                            message
                        );
                        return;
                    }

                    tracing::info!(
                        component = "config_server",
                        change = ?change,
                        kind = "GRPCRoute",
                        "Applying GRPCRoute resource change"
                    );
                    Self::execute_change_on_cache::<GRPCRoute>(change, &self.grpc_routes, resource);
                }
            }
            ResourceKind::TCPRoute => {
                if let Ok(resource) = serde_yaml::from_str::<TCPRoute>(&data) {
                    // Check if TCPRoute references a gateway that exists in base_conf
                    let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
                        if let Some(first_ref) = parent_refs.first() {
                            // Get gateway namespace (if not specified, use TCPRoute's namespace)
                            let gateway_namespace = first_ref
                                .namespace
                                .as_ref()
                                .or_else(|| resource.metadata.namespace.as_ref());
                            let gateway_name = Some(&first_ref.name);

                            let base_conf_guard = self.base_conf.read().unwrap();
                            base_conf_guard.has_gateway(gateway_namespace, gateway_name)
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if !gateway_exists {
                        let (gateway_info, message) = if let Some(parent_refs) = &resource.spec.parent_refs {
                            if let Some(first_ref) = parent_refs.first() {
                                let info = format!(
                                    "namespace={:?}, name={}",
                                    first_ref
                                        .namespace
                                        .as_ref()
                                        .or_else(|| resource.metadata.namespace.as_ref()),
                                    first_ref.name
                                );
                                (info, "TCPRoute references a Gateway that does not exist in base_conf, skipping")
                            } else {
                                (
                                    "no parent_refs".to_string(),
                                    "TCPRoute has empty parent_refs, skipping",
                                )
                            }
                        } else {
                            ("no parent_refs".to_string(), "TCPRoute has no parent_refs, skipping")
                        };

                        tracing::warn!(
                            component = "config_server",
                            change = ?change,
                            kind = "TCPRoute",
                            route_name = ?resource.metadata.name,
                            route_namespace = ?resource.metadata.namespace,
                            gateway = gateway_info,
                            "{}",
                            message
                        );
                        return;
                    }

                    tracing::info!(
                        component = "config_server",
                        change = ?change,
                        kind = "TCPRoute",
                        "Applying TCPRoute resource change"
                    );
                    Self::execute_change_on_cache::<TCPRoute>(change, &self.tcp_routes, resource);
                }
            }
            ResourceKind::UDPRoute => {
                if let Ok(resource) = serde_yaml::from_str::<UDPRoute>(&data) {
                    // Check if UDPRoute references a gateway that exists in base_conf
                    let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
                        if let Some(first_ref) = parent_refs.first() {
                            // Get gateway namespace (if not specified, use UDPRoute's namespace)
                            let gateway_namespace = first_ref
                                .namespace
                                .as_ref()
                                .or_else(|| resource.metadata.namespace.as_ref());
                            let gateway_name = Some(&first_ref.name);

                            let base_conf_guard = self.base_conf.read().unwrap();
                            base_conf_guard.has_gateway(gateway_namespace, gateway_name)
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if !gateway_exists {
                        let (gateway_info, message) = if let Some(parent_refs) = &resource.spec.parent_refs {
                            if let Some(first_ref) = parent_refs.first() {
                                let info = format!(
                                    "namespace={:?}, name={}",
                                    first_ref
                                        .namespace
                                        .as_ref()
                                        .or_else(|| resource.metadata.namespace.as_ref()),
                                    first_ref.name
                                );
                                (info, "UDPRoute references a Gateway that does not exist in base_conf, skipping")
                            } else {
                                (
                                    "no parent_refs".to_string(),
                                    "UDPRoute has empty parent_refs, skipping",
                                )
                            }
                        } else {
                            ("no parent_refs".to_string(), "UDPRoute has no parent_refs, skipping")
                        };

                        tracing::warn!(
                            component = "config_server",
                            change = ?change,
                            kind = "UDPRoute",
                            route_name = ?resource.metadata.name,
                            route_namespace = ?resource.metadata.namespace,
                            gateway = gateway_info,
                            "{}",
                            message
                        );
                        return;
                    }

                    tracing::info!(
                        component = "config_server",
                        change = ?change,
                        kind = "UDPRoute",
                        "Applying UDPRoute resource change"
                    );
                    Self::execute_change_on_cache::<UDPRoute>(change, &self.udp_routes, resource);
                }
            }
            ResourceKind::HTTPRoute => {
                if let Ok(resource) = serde_yaml::from_str::<HTTPRoute>(&data) {
                    // 检查 HTTPRoute 引用的 gateway 是否存在于 base_conf 中
                    let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
                        if let Some(first_ref) = parent_refs.first() {
                            // 获取 gateway 的 namespace（如果没有指定，使用 HTTPRoute 的 namespace）
                            let gateway_namespace = first_ref
                                .namespace
                                .as_ref()
                                .or_else(|| resource.metadata.namespace.as_ref());
                            let gateway_name = Some(&first_ref.name);

                            let base_conf_guard = self.base_conf.read().unwrap();
                            base_conf_guard.has_gateway(gateway_namespace, gateway_name)
                        } else {
                            // 没有 parent_refs，无法判断
                            false
                        }
                    } else {
                        // 没有 parent_refs，无法判断
                        false
                    };

                    if !gateway_exists {
                        let (gateway_info, message) = if let Some(parent_refs) = &resource.spec.parent_refs {
                            if let Some(first_ref) = parent_refs.first() {
                                let info = format!(
                                    "namespace={:?}, name={}",
                                    first_ref
                                        .namespace
                                        .as_ref()
                                        .or_else(|| resource.metadata.namespace.as_ref()),
                                    first_ref.name
                                );
                                (info, "HTTPRoute references a Gateway that does not exist in base_conf, skipping")
                            } else {
                                (
                                    "no parent_refs".to_string(),
                                    "HTTPRoute has empty parent_refs, skipping",
                                )
                            }
                        } else {
                            ("no parent_refs".to_string(), "HTTPRoute has no parent_refs, skipping")
                        };

                        tracing::warn!(
                            component = "config_server",
                            change = ?change,
                            kind = "HTTPRoute",
                            route_name = ?resource.metadata.name,
                            route_namespace = ?resource.metadata.namespace,
                            gateway = gateway_info,
                            "{}",
                            message
                        );
                        return;
                    }

                    tracing::info!(
                        component = "config_server",
                        change = ?change,
                        kind = "HTTPRoute",
                        "Applying HTTPRoute resource change"
                    );
                    Self::execute_change_on_cache::<HTTPRoute>(change, &self.routes, resource);
                }
            }
            ResourceKind::Service => {
                if let Ok(resource) = serde_yaml::from_str::<Service>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "Service",
                        "Applying Service resource change"
                    );
                    Self::execute_change_on_cache::<Service>(change, &self.services, resource);
                }
            }
            ResourceKind::EndpointSlice => {
                if let Ok(resource) = serde_yaml::from_str::<EndpointSlice>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "EndpointSlice",
                        "Applying EndpointSlice resource change"
                    );
                    Self::execute_change_on_cache::<EndpointSlice>(change, &self.endpoint_slices, resource);
                }
            }
            ResourceKind::EdgionTls => {
                if let Ok(resource) = serde_yaml::from_str::<EdgionTls>(&data) {
                    // 检查 EdgionTls 引用的 gateway 是否存在于 base_conf 中
                    let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
                        if let Some(first_ref) = parent_refs.first() {
                            // 获取 gateway 的 namespace（如果没有指定，使用 EdgionTls 的 namespace）
                            let gateway_namespace = first_ref
                                .namespace
                                .as_ref()
                                .or_else(|| resource.metadata.namespace.as_ref());
                            let gateway_name = Some(&first_ref.name);

                            let base_conf_guard = self.base_conf.read().unwrap();
                            base_conf_guard.has_gateway(gateway_namespace, gateway_name)
                        } else {
                            // 没有 parent_refs，无法判断
                            false
                        }
                    } else {
                        // 没有 parent_refs，无法判断
                        false
                    };

                    if !gateway_exists {
                        let (gateway_info, message) = if let Some(parent_refs) = &resource.spec.parent_refs {
                            if let Some(first_ref) = parent_refs.first() {
                                let info = format!(
                                    "namespace={:?}, name={}",
                                    first_ref
                                        .namespace
                                        .as_ref()
                                        .or_else(|| resource.metadata.namespace.as_ref()),
                                    first_ref.name
                                );
                                (
                                    info,
                                    "EdgionTls references a Gateway that does not exist in base_conf, skipping",
                                )
                            } else {
                                (
                                    "no parent_refs".to_string(),
                                    "EdgionTls has empty parent_refs, skipping",
                                )
                            }
                        } else {
                            ("no parent_refs".to_string(), "EdgionTls has no parent_refs, skipping")
                        };

                        tracing::info!(
                            component = "config_server",
                            change = ?change,
                            kind = "EdgionTls",
                            tls_name = ?resource.metadata.name,
                            tls_namespace = ?resource.metadata.namespace,
                            gateway = gateway_info,
                            "{}",
                            message
                        );
                        return;
                    }

                    tracing::info!(
                        component = "config_server",
                        kind = "EdgionTls",
                        "Applying EdgionTls resource change"
                    );
                    Self::execute_change_on_cache::<EdgionTls>(change, &self.edgion_tls, resource);
                }
            }
            ResourceKind::EdgionPlugins => {
                if let Ok(resource) = serde_yaml::from_str::<EdgionPlugins>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "EdgionPlugins",
                        "Applying EdgionPlugins resource change"
                    );
                    Self::execute_change_on_cache::<EdgionPlugins>(change, &self.edgion_plugins, resource);
                }
            }
            ResourceKind::PluginMetaData => {
                if let Ok(resource) = serde_yaml::from_str::<PluginMetaData>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "PluginMetaData",
                        metadata_name = ?resource.metadata.name,
                        data_type = ?resource.data_type(),
                        "Applying PluginMetaData resource change"
                    );
                    Self::execute_change_on_cache::<PluginMetaData>(change, &self.plugin_metadata, resource);
                }
            }
            ResourceKind::Secret => {
                if let Ok(resource) = serde_yaml::from_str::<Secret>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "Secret",
                        "Applying Secret resource change"
                    );
                    Self::execute_change_on_cache::<Secret>(change, &self.secrets, resource);
                }
            }
        }
    }
}

impl ConfigServerEventDispatcher for ConfigServer {
    fn apply_resource_change(&self, change: ResourceChange, resource_type: Option<ResourceKind>, data: String) {
        if let Some(resource_kind) = resource_type.or_else(|| ResourceKind::from_content(&data)) {
            self.apply_resource_change_with_resource_type(change, resource_kind, data)
        } else {
            tracing::error!("Resource type {:?} does not exist", resource_type);
            return;
        }
    }

    fn enable_version_fix_mode(&self) {
        self.routes.enable_version_fix_mode();
        self.grpc_routes.enable_version_fix_mode();
        self.tcp_routes.enable_version_fix_mode();
        self.udp_routes.enable_version_fix_mode();
        self.services.enable_version_fix_mode();
        self.endpoint_slices.enable_version_fix_mode();
        self.edgion_tls.enable_version_fix_mode();
        self.edgion_plugins.enable_version_fix_mode();
        self.plugin_metadata.enable_version_fix_mode();
        self.secrets.enable_version_fix_mode();
    }

    fn set_ready(&self) {
        self.routes.set_ready();
        self.grpc_routes.set_ready();
        self.tcp_routes.set_ready();
        self.udp_routes.set_ready();
        self.services.set_ready();
        self.endpoint_slices.set_ready();
        self.edgion_tls.set_ready();
        self.edgion_plugins.set_ready();
        self.plugin_metadata.set_ready();
        self.secrets.set_ready();
    }
}
