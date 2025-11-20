use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{
    ConfigServer, EventDispatch, ConfigServerEventDispatcher, ServerCache, Versionable,
};
use crate::types::{EdgionGatewayConfig, EdgionTls, Gateway, GatewayClass, HTTPRoute, ResourceKind};
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;

impl ConfigServer {
    fn execute_change_on_cache<T>(
        change: ResourceChange,
        cache: &ServerCache<T>,
        resource: T,
    ) where
        T: Clone + Send + Sync + 'static + Versionable + Resource,
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
            // Base conf resources are handled by apply_base_conf, not here
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                // This should never be reached due to the early return above,
                // but included for match exhaustiveness
                unreachable!("Base conf resources should have been handled earlier")
            }
            ResourceKind::HTTPRoute => {
                if let Ok(resource) = serde_yaml::from_str::<HTTPRoute>(&data) {
                    // 检查 HTTPRoute 引用的 gateway 是否存在于 base_conf 中
                    let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
                        if let Some(first_ref) = parent_refs.first() {
                            // 获取 gateway 的 namespace（如果没有指定，使用 HTTPRoute 的 namespace）
                            let gateway_namespace = first_ref.namespace.as_ref()
                                .or_else(|| resource.metadata.namespace.as_ref());
                            let gateway_name = Some(&first_ref.name);

                            let base_conf = self.base_conf.read().unwrap();
                            base_conf.has_gateway(gateway_namespace, gateway_name)
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
                                    first_ref.namespace.as_ref().or_else(|| resource.metadata.namespace.as_ref()),
                                    first_ref.name
                                );
                                (info, "HTTPRoute references a Gateway that does not exist in base_conf, skipping")
                            } else {
                                ("no parent_refs".to_string(), "HTTPRoute has empty parent_refs, skipping")
                            }
                        } else {
                            ("no parent_refs".to_string(), "HTTPRoute has no parent_refs, skipping")
                        };

                        tracing::info!(
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
                    Self::execute_change_on_cache::<HTTPRoute>(
                        change,
                        &self.routes,
                        resource,
                    );
                }
            }
            ResourceKind::Service => {
                if let Ok(resource) = serde_yaml::from_str::<Service>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "Service",
                        "Applying Service resource change"
                    );
                    Self::execute_change_on_cache::<Service>(
                        change,
                        &self.services,
                        resource,
                    );
                }
            }
            ResourceKind::EndpointSlice => {
                if let Ok(resource) = serde_yaml::from_str::<EndpointSlice>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "EndpointSlice",
                        "Applying EndpointSlice resource change"
                    );
                    Self::execute_change_on_cache::<EndpointSlice>(
                        change,
                        &self.endpoint_slices,
                        resource,
                    );
                }
            }
            ResourceKind::EdgionTls => {
                if let Ok(resource) = serde_yaml::from_str::<EdgionTls>(&data) {
                    // 检查 EdgionTls 引用的 gateway 是否存在于 base_conf 中
                    let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
                        if let Some(first_ref) = parent_refs.first() {
                            // 获取 gateway 的 namespace（如果没有指定，使用 EdgionTls 的 namespace）
                            let gateway_namespace = first_ref.namespace.as_ref()
                                .or_else(|| resource.metadata.namespace.as_ref());
                            let gateway_name = Some(&first_ref.name);

                            let base_conf = self.base_conf.read().unwrap();
                            base_conf.has_gateway(gateway_namespace, gateway_name)
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
                                    first_ref.namespace.as_ref().or_else(|| resource.metadata.namespace.as_ref()),
                                    first_ref.name
                                );
                                (info, "EdgionTls references a Gateway that does not exist in base_conf, skipping")
                            } else {
                                ("no parent_refs".to_string(), "EdgionTls has empty parent_refs, skipping")
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
                    Self::execute_change_on_cache::<EdgionTls>(
                        change,
                        &self.edgion_tls,
                        resource,
                    );
                }
            }
            ResourceKind::Secret => {
                if let Ok(resource) = serde_yaml::from_str::<Secret>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "Secret",
                        "Applying Secret resource change"
                    );
                    Self::execute_change_on_cache::<Secret>(
                        change,
                        &self.secrets,
                        resource,
                    );
                }
            }
        }
    }

    fn apply_base_conf_with_resource_type(
        &self,
        change: ResourceChange,
        resource_type: ResourceKind,
        data: String,
    ) {
        // Only process base conf resources
        match resource_type {
            ResourceKind::GatewayClass => {
                if let Ok(resource) = serde_yaml::from_str::<GatewayClass>(&data) {
                    // Filter by configured gateway class name
                    if let Some(configured_gc) = &self.gateway_class {
                        if let Some(name) = &resource.metadata.name {
                            if name != configured_gc {
                                tracing::debug!(
                                    component = "config_server",
                                    event = "skip_gateway_class_mismatch",
                                    configured = %configured_gc,
                                    received = %name,
                                    "Skipping GatewayClass mismatch"
                                );
                                return;
                            }
                        }
                    }

                    tracing::info!(
                        component = "config_server",
                        kind = "GatewayClass",
                        event = "apply_base_conf",
                        gateway_class_name = ?resource.metadata.name,
                        change = ?change,
                        "Applying GatewayClass to base_conf"
                    );
                    match change {
                        ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                            let mut base_conf = self.base_conf.write().unwrap();

                            // Check if existing EdgionGatewayConfig matches the new GatewayClass
                            // If GatewayClass has parametersRef, validate the match
                            // If GatewayClass has no parametersRef, clear any existing config
                            if let Some(existing_config) = base_conf.edgion_gateway_config() {
                                let config_name = existing_config.metadata.name.as_deref().unwrap_or("");
                                let should_keep = if let Some(ref params) = resource.spec.parameters_ref {
                                    // Check group, kind, and name
                                    params.group == "example.com" &&
                                        params.kind == "EdgionGatewayConfig" &&
                                        params.name == config_name &&
                                        // EdgionGatewayConfig is cluster-scoped, so namespace should be None
                                        params.namespace.is_none()
                                } else {
                                    // GatewayClass has no parametersRef, so no config should exist
                                    false
                                };

                                if !should_keep {
                                    tracing::info!(
                                        component = "config_server",
                                        event = "clear_mismatched_config",
                                        config_name = %config_name,
                                        "Clearing EdgionGatewayConfig that does not match new GatewayClass parametersRef"
                                    );
                                    base_conf.clear_edgion_gateway_config();
                                }
                            }

                            base_conf.set_gateway_class(resource);
                        }
                        ResourceChange::EventDelete => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.clear_gateway_class();
                            // Also clear config as it's no longer referenced
                            base_conf.clear_edgion_gateway_config();
                        }
                    }
                }
            }
            ResourceKind::EdgionGatewayConfig => {
                if let Ok(resource) = serde_yaml::from_str::<EdgionGatewayConfig>(&data) {
                    // Check against existing GatewayClass
                    let should_load = {
                        let base_conf = self.base_conf.read().unwrap();
                        if let Some(gc) = base_conf.gateway_class() {
                            if let Some(ref params) = gc.spec.parameters_ref {
                                // Check group, kind, name, and namespace
                                params.group == "example.com" &&
                                    params.kind == "EdgionGatewayConfig" &&
                                    params.name == resource.metadata.name.as_deref().unwrap_or("") &&
                                    // EdgionGatewayConfig is cluster-scoped, so namespace should be None
                                    params.namespace.is_none()
                            } else {
                                // GatewayClass has no parametersRef, so no config should be loaded
                                false
                            }
                        } else {
                            // GatewayClass not loaded yet
                            // If gateway_class is configured, we should wait for the matching GatewayClass
                            // If gateway_class is not configured, allow loading for now (will be validated later)
                            if self.gateway_class.is_some() {
                                // GatewayClass is configured but not loaded yet (probably skipped due to mismatch)
                                // Don't load EdgionGatewayConfig
                                false
                            } else {
                                // No gateway_class filter, allow loading for now
                                // It will be validated when GatewayClass is loaded (reverse check)
                                true
                            }
                        }
                    };

                    if !should_load {
                        tracing::debug!(
                            component = "config_server",
                            event = "skip_config_mismatch",
                            config_name = ?resource.metadata.name,
                            "Skipping EdgionGatewayConfig not referenced by current GatewayClass"
                        );
                        return;
                    }

                    tracing::info!(
                        component = "config_server",
                        kind = "EdgionGatewayConfig",
                        event = "apply_base_conf",
                        edgion_gateway_config_name = ?resource.metadata.name,
                        change = ?change,
                        "Applying EdgionGatewayConfig to base_conf"
                    );
                    match change {
                        ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.set_edgion_gateway_config(resource);
                        }
                        ResourceChange::EventDelete => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.clear_edgion_gateway_config();
                        }
                    }
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_yaml::from_str::<Gateway>(&data) {
                    // Filter by gateway class name in spec
                    if let Some(configured_gc) = &self.gateway_class {
                        if resource.spec.gateway_class_name != *configured_gc {
                            tracing::debug!(
                                component = "config_server",
                                event = "skip_gateway_class_mismatch",
                                configured = %configured_gc,
                                received = %resource.spec.gateway_class_name,
                                "Skipping Gateway with mismatched gatewayClassName"
                            );
                            return;
                        }
                    }

                    tracing::info!(
                        component = "config_server",
                        kind = "Gateway",
                        event = "apply_base_conf",
                        gateway_name = ?resource.metadata.name,
                        gateway_namespace = ?resource.metadata.namespace,
                        change = ?change,
                        "Applying Gateway to base_conf"
                    );
                    match change {
                        ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.add_gateway(resource);
                        }
                        ResourceChange::EventDelete => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.remove_gateway(
                                resource.metadata.namespace.as_ref(),
                                resource.metadata.name.as_ref()
                            );
                        }
                    }
                }
            }
            _ => {
                tracing::warn!(
                    component = "config_server",
                    event = "invalid_resource_type_for_base_conf",
                    resource_type = ?resource_type,
                    "apply_base_conf called with non-base-conf resource type"
                );
            }
        }
    }

}

impl ConfigServerEventDispatcher for ConfigServer {

    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
    ) {
        if let Some(resource_kind) = resource_type.or_else(|| ResourceKind::from_content(&data)) {
            self.apply_resource_change_with_resource_type(
                change,
                resource_kind,
                data,
            )
        } else {
            tracing::error!("Resource type {:?} does not exist", resource_type);
            return;
        }
    }


    fn apply_base_conf(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
    ) {
        if let Some(resource_kind) = resource_type.or_else(|| ResourceKind::from_content(&data)) {
            self.apply_base_conf_with_resource_type(
                change,
                resource_kind,
                data,
            )
        } else {
            tracing::error!("Resource type {:?} does not exist", resource_type);
            return;
        }
    }

    fn enable_version_fix_mode(&self) {
        self.routes.enable_version_fix_mode();
        self.services.enable_version_fix_mode();
        self.endpoint_slices.enable_version_fix_mode();
        self.edgion_tls.enable_version_fix_mode();
        self.secrets.enable_version_fix_mode();
    }

    fn set_ready(&self) {
        self.routes.set_ready();
        self.services.set_ready();
        self.endpoint_slices.set_ready();
        self.edgion_tls.set_ready();
        self.secrets.set_ready();
    }
}
