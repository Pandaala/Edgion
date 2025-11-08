use crate::core::conf_sync::config_center::{GatewayClassKey, ResourceItem};
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{
    CenterCache, ConfigCenter, EventDispatch, EventDispatcher, Versionable,
};
use crate::types::{
    EdgionGatewayConfig, EdgionTls, Gateway, GatewayClass, HTTPRoute, ResourceKind,
};
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::HashMap;

const DEFAULT_GATEWAY_CLASS_KEY: &str = "default";

impl ConfigCenter {
    fn execute_change_on_cache<T>(
        change: ResourceChange,
        cache: &mut CenterCache<T>,
        resource: T,
        resource_version: Option<u64>,
    ) where
        T: Clone + Send + Sync + 'static + Versionable,
    {
        match change {
            ResourceChange::InitAdd => cache.init_add(resource, resource_version),
            ResourceChange::EventAdd => cache.event_add(resource, resource_version),
            ResourceChange::EventUpdate => cache.event_update(resource, resource_version),
            ResourceChange::EventDelete => cache.event_del(resource, resource_version),
        }
    }

    fn fallback_gateway_class_keys(&self) -> Vec<String> {
        if self.gateway_classes.is_empty() {
            vec![DEFAULT_GATEWAY_CLASS_KEY.to_string()]
        } else {
            self.gateway_classes.keys().cloned().collect()
        }
    }

    fn resolve_gateway_class_keys_for_item(&self, resource: &ResourceItem) -> Vec<String> {
        match resource {
            ResourceItem::GatewayClass(resource) => resource
                .metadata
                .name
                .clone()
                .map(|key| vec![key])
                .unwrap_or_else(|| vec![DEFAULT_GATEWAY_CLASS_KEY.to_string()]),
            ResourceItem::Gateway(resource) => vec![resource.spec.gateway_class_name.clone()],
            _ => self.fallback_gateway_class_keys(),
        }
    }

    fn apply_change_to_cache<T>(
        map: &mut HashMap<GatewayClassKey, CenterCache<T>>,
        key: GatewayClassKey,
        change: ResourceChange,
        resource: T,
        resource_version: Option<u64>,
    ) where
        T: Clone + Send + Sync + 'static + Versionable,
    {
        match change {
            ResourceChange::EventUpdate | ResourceChange::EventDelete => {
                if let Some(cache) = map.get_mut(&key) {
                    Self::execute_change_on_cache(
                        change,
                        cache,
                        resource.clone(),
                        resource_version,
                    );
                }
            }
            _ => {
                let cache = map.entry(key).or_insert_with(|| CenterCache::new(1000));
                Self::execute_change_on_cache(change, cache, resource.clone(), resource_version);
            }
        }
    }
}

impl EventDispatcher for ConfigCenter {
    fn apply_resource_change(
        &mut self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            return;
        };

        let result = match resource_type {
            ResourceKind::GatewayClass => {
                serde_json::from_str::<GatewayClass>(&data).map(|resource| {
                    let item = ResourceItem::GatewayClass(resource);
                    let gateway_class_keys = self.resolve_gateway_class_keys_for_item(&item);
                    for key in gateway_class_keys {
                        if let Some(cache) = self.gateway_classes.get_mut(&key) {
                            Self::execute_change_on_cache::<GatewayClass>(change, cache, resource, resource_version);
                        }
                    }
                })
            }
            ResourceKind::EdgionGatewayConfig => {
                serde_json::from_str::<EdgionGatewayConfig>(&data).map(|resource| {
                    let item = ResourceItem::EdgionGatewayConfig(resource);
                    let gateway_class_keys = self.resolve_gateway_class_keys_for_item(&item);
                    for key in gateway_class_keys {
                        if let Some(cache) = self.edgion_gateway_configs.get_mut(&key) {
                            Self::execute_change_on_cache::<EdgionGatewayConfig>(change, cache, resource, resource_version);
                        }
                    }
                })
            }
            ResourceKind::Gateway => {
                serde_json::from_str::<Gateway>(&data).map(|resource| {
                    let item = ResourceItem::Gateway(resource);
                    let gateway_class_keys = self.resolve_gateway_class_keys_for_item(&item);
                    for key in gateway_class_keys {
                        if let Some(cache) = self.gateways.get_mut(&key) {
                            Self::execute_change_on_cache::<Gateway>(change, cache, resource, resource_version);
                        }
                    }
                })
            }
            ResourceKind::HTTPRoute => {
                serde_json::from_str::<HTTPRoute>(&data).map(|resource| {
                    let item = ResourceItem::HTTPRoute(resource);
                    let gateway_class_keys = self.resolve_gateway_class_keys_for_item(&item);
                    for key in gateway_class_keys {
                        if let Some(cache) = self.routes.get_mut(&key) {
                            Self::execute_change_on_cache::<HTTPRoute>(change, cache, resource, resource_version);
                        }
                    }
                })
            }
            ResourceKind::Service => {
                serde_json::from_str::<Service>(&data).map(|resource| {
                    let item = ResourceItem::Service(resource);
                    let gateway_class_keys = self.resolve_gateway_class_keys_for_item(&item);
                    for key in gateway_class_keys {
                        if let Some(cache) = self.services.get_mut(&key) {
                            Self::execute_change_on_cache::<Service>(change, cache, resource, resource_version);
                        }
                    }
                })
            }
            ResourceKind::EndpointSlice => {
                serde_json::from_str::<EndpointSlice>(&data).map(|resource| {
                    let item = ResourceItem::EndpointSlice(resource);
                    let gateway_class_keys = self.resolve_gateway_class_keys_for_item(&item);
                    for key in gateway_class_keys {
                        if let Some(cache) = self.endpoint_slices.get_mut(&key) {
                            Self::execute_change_on_cache::<EndpointSlice>(change, cache, resource, resource_version);
                        }
                    }
                })
            }
            ResourceKind::EdgionTls => {
                serde_json::from_str::<EdgionTls>(&data).map(|resource| {
                    let item = ResourceItem::EdgionTls(resource);
                    let gateway_class_keys = self.resolve_gateway_class_keys_for_item(&item);
                    for key in gateway_class_keys {
                        if let Some(cache) = self.edgion_tls.get_mut(&key) {
                            Self::execute_change_on_cache::<EdgionTls>(change, cache, resource, resource_version);
                        }
                    }
                })
            }
            ResourceKind::Secret => {
                serde_json::from_str::<Secret>(&data).map(|resource| {
                    let item = ResourceItem::Secret(resource);
                    let gateway_class_keys = self.resolve_gateway_class_keys_for_item(&item);
                    for key in gateway_class_keys {
                        if let Some(cache) = self.secrets.get_mut(&key) {
                            Self::execute_change_on_cache::<Secret>(change, cache, resource, resource_version);
                        }
                    }
                })
            }
        };

        if let Err(err) = result {
            eprintln!(
                "[ConfigCenter::apply_resource_change] Failed to parse resource {:?}: {} (data: {})",
                resource_type,
                err,
                &data[..data.len().min(200)]
            );
        }
    }

    fn set_ready(&mut self) {
        for cache in self.gateway_classes.values_mut() {
            cache.set_ready();
        }
        for cache in self.edgion_gateway_configs.values_mut() {
            cache.set_ready();
        }
        for cache in self.gateways.values_mut() {
            cache.set_ready();
        }
        for cache in self.routes.values_mut() {
            cache.set_ready();
        }
        for cache in self.services.values_mut() {
            cache.set_ready();
        }
        for cache in self.endpoint_slices.values_mut() {
            cache.set_ready();
        }
        for cache in self.edgion_tls.values_mut() {
            cache.set_ready();
        }
        for cache in self.secrets.values_mut() {
            cache.set_ready();
        }
    }
}
