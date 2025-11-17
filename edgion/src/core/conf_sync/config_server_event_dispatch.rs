use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{
    ConfigServer, EventDispatch, EventDispatcher, ServerCache, Versionable,
};
use crate::types::{EdgionGatewayConfig, EdgionTls, Gateway, GatewayClass, HTTPRoute, ResourceKind};
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;

const DEFAULT_GATEWAY_CLASS_KEY: &str = "default";

trait ResolveGatewayClassKeysForItem {
    fn resolve_gateway_class_keys_for_item(&self, center: &ConfigServer) -> Vec<String>;
}

impl ResolveGatewayClassKeysForItem for GatewayClass {
    fn resolve_gateway_class_keys_for_item(&self, _center: &ConfigServer) -> Vec<String> {
        self.metadata
            .name
            .clone()
            .map(|key| vec![key])
            .unwrap_or_else(|| vec![DEFAULT_GATEWAY_CLASS_KEY.to_string()])
    }
}

impl ResolveGatewayClassKeysForItem for EdgionGatewayConfig {
    fn resolve_gateway_class_keys_for_item(&self, center: &ConfigServer) -> Vec<String> {
        self.metadata
            .name
            .clone()
            .map(|key| vec![key])
            .unwrap_or_else(|| center.fallback_gateway_class_keys())
    }
}

impl ResolveGatewayClassKeysForItem for Gateway {
    fn resolve_gateway_class_keys_for_item(&self, _center: &ConfigServer) -> Vec<String> {
        vec![self.spec.gateway_class_name.clone()]
    }
}

impl ResolveGatewayClassKeysForItem for HTTPRoute {
    fn resolve_gateway_class_keys_for_item(&self, center: &ConfigServer) -> Vec<String> {
        center.fallback_gateway_class_keys()
    }
}

impl ResolveGatewayClassKeysForItem for Service {
    fn resolve_gateway_class_keys_for_item(&self, center: &ConfigServer) -> Vec<String> {
        center.fallback_gateway_class_keys()
    }
}

impl ResolveGatewayClassKeysForItem for EndpointSlice {
    fn resolve_gateway_class_keys_for_item(&self, center: &ConfigServer) -> Vec<String> {
        center.fallback_gateway_class_keys()
    }
}

impl ResolveGatewayClassKeysForItem for EdgionTls {
    fn resolve_gateway_class_keys_for_item(&self, center: &ConfigServer) -> Vec<String> {
        center.fallback_gateway_class_keys()
    }
}

impl ResolveGatewayClassKeysForItem for Secret {
    fn resolve_gateway_class_keys_for_item(&self, center: &ConfigServer) -> Vec<String> {
        center.fallback_gateway_class_keys()
    }
}

impl ConfigServer {
    fn execute_change_on_cache<T>(
        change: ResourceChange,
        cache: &mut ServerCache<T>,
        resource: T,
        resource_version: Option<u64>,
    ) where
        T: Clone + Send + Sync + 'static + Versionable + Resource,
    {
        cache.apply_change(change, resource);
    }

    fn fallback_gateway_class_keys(&self) -> Vec<String> {
        let gateway_classes = self.gateway_classes.read().unwrap();
        if gateway_classes.is_empty() {
            vec![DEFAULT_GATEWAY_CLASS_KEY.to_string()]
        } else {
            gateway_classes.keys().cloned().collect()
        }
    }
}

impl EventDispatcher for ConfigServer {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {

        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        
        if resource_type.is_none() {
            tracing::warn!(
                component = "config_server",
                event = "unknown_resource_type",
                data_preview = %data.chars().take(500).collect::<String>(),
                "Failed to determine resource type from content"
            );
            return;
        }
        
        let resource_type = resource_type.unwrap();

        // Skip base conf resources - they should be handled by apply_base_conf
        match resource_type {
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                tracing::debug!(
                    component = "config_server",
                    event = "skip_base_conf_in_apply_resource_change",
                    resource_type = ?resource_type,
                    "Base conf resources should be handled by apply_base_conf, skipping apply_resource_change"
                );
                return;
            }
            _ => {}
        }

        match resource_type {
            // Base conf resources are handled by apply_base_conf, not here
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                // This should never be reached due to the early return above,
                // but included for match exhaustiveness
                unreachable!("Base conf resources should have been handled earlier")
            }
            ResourceKind::HTTPRoute => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    let gateway_class_keys = resource.resolve_gateway_class_keys_for_item(self);
                    tracing::info!(
                        component = "config_server",
                        kind = "HTTPRoute",
                        gateway_class_keys = ?gateway_class_keys,
                    );
                    let mut routes = self.routes.write().unwrap();
                    for key in gateway_class_keys {
                        let cache = routes
                            .entry(key.clone())
                            .or_insert_with(|| ServerCache::new(200));
                        
                        Self::execute_change_on_cache::<HTTPRoute>(
                            change,
                            cache,
                            resource.clone(),
                            resource_version,
                        );
                    }
                }
            }
            ResourceKind::Service => {
                if let Ok(resource) = serde_json::from_str::<Service>(&data) {
                    let gateway_class_keys = resource.resolve_gateway_class_keys_for_item(self);
                    tracing::info!(
                        component = "config_server",
                        kind = "Service",
                        gateway_class_keys = ?gateway_class_keys,
                    );
                    let mut services = self.services.write().unwrap();
                    for key in gateway_class_keys {
                        let cache = services
                            .entry(key.clone())
                            .or_insert_with(|| ServerCache::new(200));
                        
                        Self::execute_change_on_cache::<Service>(
                            change,
                            cache,
                            resource.clone(),
                            resource_version,
                        );
                    }
                }
            }
            ResourceKind::EndpointSlice => {
                if let Ok(resource) = serde_json::from_str::<EndpointSlice>(&data) {
                    let gateway_class_keys = resource.resolve_gateway_class_keys_for_item(self);
                    tracing::info!(
                        component = "config_server",
                        kind = "EndpointSlice",
                        gateway_class_keys = ?gateway_class_keys,
                    );
                    let mut endpoint_slices = self.endpoint_slices.write().unwrap();
                    for key in gateway_class_keys {
                        let cache = endpoint_slices
                            .entry(key.clone())
                            .or_insert_with(|| ServerCache::new(200));
                        
                        Self::execute_change_on_cache::<EndpointSlice>(
                            change,
                            cache,
                            resource.clone(),
                            resource_version,
                        );
                    }
                }
            }
            ResourceKind::EdgionTls => {
                if let Ok(resource) = serde_json::from_str::<EdgionTls>(&data) {
                    let gateway_class_keys = resource.resolve_gateway_class_keys_for_item(self);
                    let mut edgion_tls = self.edgion_tls.write().unwrap();
                    tracing::info!(
                        component = "config_server",
                        kind = "EdgionTls",
                        gateway_class_keys = ?gateway_class_keys,
                    );
                    for key in gateway_class_keys {
                        let cache = edgion_tls
                            .entry(key.clone())
                            .or_insert_with(|| ServerCache::new(200));
                        
                        Self::execute_change_on_cache::<EdgionTls>(
                            change,
                            cache,
                            resource.clone(),
                            resource_version,
                        );
                    }
                }
            }
            ResourceKind::Secret => {
                if let Ok(resource) = serde_json::from_str::<Secret>(&data) {
                    let gateway_class_keys = resource.resolve_gateway_class_keys_for_item(self);
                    let mut secrets = self.secrets.write().unwrap();
                    tracing::info!(
                        component = "config_server",
                        kind = "Secret",
                        gateway_class_keys = ?gateway_class_keys,
                    );
                    for key in gateway_class_keys {
                        let cache = secrets
                            .entry(key.clone())
                            .or_insert_with(|| ServerCache::new(200));
                        
                        Self::execute_change_on_cache::<Secret>(
                            change,
                            cache,
                            resource.clone(),
                            resource_version,
                        );
                    }
                }
            }
        }
    }

    fn set_ready(&self) {
        use crate::core::conf_sync::EventDispatch;

        let mut gateway_classes = self.gateway_classes.write().unwrap();
        for cache in gateway_classes.values_mut() {
            cache.set_ready();
        }

        let mut edgion_gateway_configs = self.edgion_gateway_configs.write().unwrap();
        for cache in edgion_gateway_configs.values_mut() {
            cache.set_ready();
        }

        let mut gateways = self.gateways.write().unwrap();
        for cache in gateways.values_mut() {
            cache.set_ready();
        }

        let mut routes = self.routes.write().unwrap();
        for cache in routes.values_mut() {
            cache.set_ready();
        }

        let mut services = self.services.write().unwrap();
        for cache in services.values_mut() {
            cache.set_ready();
        }

        let mut endpoint_slices = self.endpoint_slices.write().unwrap();
        for cache in endpoint_slices.values_mut() {
            cache.set_ready();
        }

        let mut edgion_tls = self.edgion_tls.write().unwrap();
        for cache in edgion_tls.values_mut() {
            cache.set_ready();
        }

        let mut secrets = self.secrets.write().unwrap();
        for cache in secrets.values_mut() {
            cache.set_ready();
        }
    }

    fn apply_base_conf(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        _resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        
        if resource_type.is_none() {
            tracing::warn!(
                component = "config_server",
                event = "unknown_resource_type",
                data_preview = %data.chars().take(500).collect::<String>(),
                "Failed to determine resource type from content in apply_base_conf"
            );
            return;
        }
        
        let resource_type = resource_type.unwrap();

        // Only process base conf resources
        match resource_type {
            ResourceKind::GatewayClass => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
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
                            base_conf.set_gateway_class(resource);
                        }
                        ResourceChange::EventDelete => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.clear_gateway_class();
                        }
                    }
                }
            }
            ResourceKind::EdgionGatewayConfig => {
                if let Ok(resource) = serde_json::from_str::<EdgionGatewayConfig>(&data) {
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
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
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
