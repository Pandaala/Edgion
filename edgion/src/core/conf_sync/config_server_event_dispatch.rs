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
        tracing::debug!(
            component = "config_server",
            event = "resource_change",
            change = ?change,
            resource_type = ?resource_type,
            resource_version = ?resource_version,
            data_preview = %data.chars().take(200).collect::<String>(),
            "Applying resource change"
        );

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

        match resource_type {
            ResourceKind::GatewayClass => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    let gateway_class_keys = resource.resolve_gateway_class_keys_for_item(self);
                    let mut gateway_classes = self.gateway_classes.write().unwrap();
                    for key in gateway_class_keys {
                        let cache = gateway_classes
                            .entry(key.clone())
                            .or_insert_with(|| ServerCache::new(200));
                        
                        Self::execute_change_on_cache::<GatewayClass>(
                            change,
                            cache,
                            resource.clone(),
                            resource_version,
                        );
                    }
                }
            }
            ResourceKind::EdgionGatewayConfig => {
                if let Ok(resource) = serde_json::from_str::<EdgionGatewayConfig>(&data) {
                    let gateway_class_keys = resource.resolve_gateway_class_keys_for_item(self);
                    let mut edgion_gateway_configs = self.edgion_gateway_configs.write().unwrap();
                    for key in gateway_class_keys {
                        let cache = edgion_gateway_configs
                            .entry(key.clone())
                            .or_insert_with(|| ServerCache::new(200));
                        
                        Self::execute_change_on_cache::<EdgionGatewayConfig>(
                            change,
                            cache,
                            resource.clone(),
                            resource_version,
                        );
                    }
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    let gateway_class_keys = resource.resolve_gateway_class_keys_for_item(self);
                    let mut gateways = self.gateways.write().unwrap();
                    for key in gateway_class_keys {
                        let cache = gateways
                            .entry(key.clone())
                            .or_insert_with(|| ServerCache::new(200));
                        
                        Self::execute_change_on_cache::<Gateway>(
                            change,
                            cache,
                            resource.clone(),
                            resource_version,
                        );
                    }
                }
            }
            ResourceKind::HTTPRoute => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    let gateway_class_keys = resource.resolve_gateway_class_keys_for_item(self);
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
}
