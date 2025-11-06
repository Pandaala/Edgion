use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::core::conf_sync::traits::EventDispatcher;
use crate::core::conf_sync::watcher_cache::{EventDispatch, ListData, WatchResponse, WatcherCache};
use crate::types::{EdgionTls, Gateway, GatewayClass, GatewayClassSpec, HTTPRoute};

pub type GatewayClassKey = String;

#[derive(Debug, Clone, Copy)]
pub enum ResourceType {
    GatewayClass,
    Gateway,
    HTTPRoute,
    Service,
    EndpointSlice,
    EdgionTls,
    Secret,
}

pub struct WatcherMgr {
    gateway_classes: HashMap<GatewayClassKey, WatcherCache<GatewayClass>>,
    gateway_class_specs: HashMap<GatewayClassKey, WatcherCache<GatewayClassSpec>>,
    gateways: HashMap<String, WatcherCache<Gateway>>,
    routes: HashMap<GatewayClassKey, WatcherCache<HTTPRoute>>,
    services: HashMap<GatewayClassKey, WatcherCache<Service>>,
    endpoint_slices: HashMap<GatewayClassKey, WatcherCache<EndpointSlice>>,
    edgion_tls: HashMap<GatewayClassKey, WatcherCache<EdgionTls>>,
    secrets: HashMap<GatewayClassKey, WatcherCache<Secret>>,
}

impl WatcherMgr {
    pub fn new() -> Self {
        Self {
            gateway_classes: HashMap::new(),
            gateway_class_specs: HashMap::new(),
            gateways: HashMap::new(),
            routes: HashMap::new(),
            services: HashMap::new(),
            endpoint_slices: HashMap::new(),
            edgion_tls: HashMap::new(),
            secrets: HashMap::new(),
        }
    }

    /// List gateway classes
    pub fn list_gateway_classes(&self, key: &str) -> Option<ListData<&GatewayClass>> {
        self.gateway_classes.get(key).map(|cache| cache.list())
    }

    /// List gateway class specs
    pub fn list_gateway_class_specs(&self, key: &str) -> Option<ListData<&GatewayClassSpec>> {
        self.gateway_class_specs.get(key).map(|cache| cache.list())
    }

    /// List gateways
    pub fn list_gateways(&self, key: &str) -> Option<ListData<&Gateway>> {
        self.gateways.get(key).map(|cache| cache.list())
    }

    /// List HTTP routes
    pub fn list_routes(&self, key: &str) -> Option<ListData<&HTTPRoute>> {
        self.routes.get(key).map(|cache| cache.list())
    }

    /// List services
    pub fn list_services(&self, key: &str) -> Option<ListData<&Service>> {
        self.services.get(key).map(|cache| cache.list())
    }

    /// List endpoint slices
    pub fn list_endpoint_slices(&self, key: &str) -> Option<ListData<&EndpointSlice>> {
        self.endpoint_slices.get(key).map(|cache| cache.list())
    }

    /// List Edgion TLS
    pub fn list_edgion_tls(&self, key: &str) -> Option<ListData<&EdgionTls>> {
        self.edgion_tls.get(key).map(|cache| cache.list())
    }

    /// List secrets
    pub fn list_secrets(&self, key: &str) -> Option<ListData<&Secret>> {
        self.secrets.get(key).map(|cache| cache.list())
    }

    /// Watch gateway classes
    pub fn watch_gateway_classes(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<GatewayClass>>> {
        self.gateway_classes
            .get_mut(key)
            .map(|cache| cache.watch(client_id, client_name, from_version))
    }

    /// Watch gateway class specs
    pub fn watch_gateway_class_specs(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<GatewayClassSpec>>> {
        self.gateway_class_specs
            .get_mut(key)
            .map(|cache| cache.watch(client_id, client_name, from_version))
    }

    /// Watch gateways
    pub fn watch_gateways(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<Gateway>>> {
        self.gateways
            .get_mut(key)
            .map(|cache| cache.watch(client_id, client_name, from_version))
    }

    /// Watch HTTP routes
    pub fn watch_routes(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<HTTPRoute>>> {
        self.routes
            .get_mut(key)
            .map(|cache| cache.watch(client_id, client_name, from_version))
    }

    /// Watch services
    pub fn watch_services(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<Service>>> {
        self.services
            .get_mut(key)
            .map(|cache| cache.watch(client_id, client_name, from_version))
    }

    /// Watch endpoint slices
    pub fn watch_endpoint_slices(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<EndpointSlice>>> {
        self.endpoint_slices
            .get_mut(key)
            .map(|cache| cache.watch(client_id, client_name, from_version))
    }

    /// Watch Edgion TLS
    pub fn watch_edgion_tls(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<EdgionTls>>> {
        self.edgion_tls
            .get_mut(key)
            .map(|cache| cache.watch(client_id, client_name, from_version))
    }

    /// Watch secrets
    pub fn watch_secrets(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<Secret>>> {
        self.secrets
            .get_mut(key)
            .map(|cache| cache.watch(client_id, client_name, from_version))
    }
}

impl Default for WatcherMgr {
    fn default() -> Self {
        Self::new()
    }
}

impl EventDispatcher for WatcherMgr {
    fn init_add(&mut self, resource_type: &str, data: String, resource_version: Option<u64>) {
        match resource_type {
            "GatewayClass" => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    if let Some(key) = resource.metadata.name.clone() {
                        let cache = self
                            .gateway_classes
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.init_add(resource, resource_version);
                    }
                }
            }
            "GatewayClassSpec" => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    let key = "default".to_string();
                    let cache = self
                        .gateway_class_specs
                        .entry(key)
                        .or_insert_with(|| WatcherCache::new(1000));
                    cache.init_add(resource, resource_version);
                }
            }
            "Gateway" => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    let key = resource.spec.gateway_class_name.clone();
                    let cache = self
                        .gateways
                        .entry(key)
                        .or_insert_with(|| WatcherCache::new(1000));
                    cache.init_add(resource, resource_version);
                }
            }
            "HTTPRoute" => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    if let Some(key) = resource
                        .spec
                        .parent_refs
                        .as_ref()
                        .and_then(|refs| refs.first())
                        .map(|parent| parent.name.clone())
                    {
                        let cache = self
                            .routes
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.init_add(resource, resource_version);
                    }
                }
            }
            "Service" => {
                if let Ok(resource) = serde_json::from_str::<Service>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        let cache = self
                            .services
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.init_add(resource, resource_version);
                    }
                }
            }
            "EndpointSlice" => {
                if let Ok(resource) = serde_json::from_str::<EndpointSlice>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        let cache = self
                            .endpoint_slices
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.init_add(resource, resource_version);
                    }
                }
            }
            "EdgionTls" => {
                if let Ok(resource) = serde_json::from_str::<EdgionTls>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        let cache = self
                            .edgion_tls
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.init_add(resource, resource_version);
                    }
                }
            }
            "Secret" => {
                if let Ok(resource) = serde_json::from_str::<Secret>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        let cache = self
                            .secrets
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.init_add(resource, resource_version);
                    }
                }
            }
            _ => {}
        }
    }

    fn set_ready(&mut self) {
        for cache in self.gateway_classes.values_mut() {
            cache.set_ready();
        }
        for cache in self.gateway_class_specs.values_mut() {
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

    fn event_add(&mut self, resource_type: &str, data: String, resource_version: Option<u64>) {
        match resource_type {
            "GatewayClass" => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    if let Some(key) = resource.metadata.name.clone() {
                        let cache = self
                            .gateway_classes
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.event_add(resource, resource_version);
                    }
                }
            }
            "GatewayClassSpec" => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    let key = "default".to_string();
                    let cache = self
                        .gateway_class_specs
                        .entry(key)
                        .or_insert_with(|| WatcherCache::new(1000));
                    cache.event_add(resource, resource_version);
                }
            }
            "Gateway" => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    let key = resource.spec.gateway_class_name.clone();
                    let cache = self
                        .gateways
                        .entry(key)
                        .or_insert_with(|| WatcherCache::new(1000));
                    cache.event_add(resource, resource_version);
                }
            }
            "HTTPRoute" => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    if let Some(key) = resource
                        .spec
                        .parent_refs
                        .as_ref()
                        .and_then(|refs| refs.first())
                        .map(|parent| parent.name.clone())
                    {
                        let cache = self
                            .routes
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.event_add(resource, resource_version);
                    }
                }
            }
            "Service" => {
                if let Ok(resource) = serde_json::from_str::<Service>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        let cache = self
                            .services
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.event_add(resource, resource_version);
                    }
                }
            }
            "EndpointSlice" => {
                if let Ok(resource) = serde_json::from_str::<EndpointSlice>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        let cache = self
                            .endpoint_slices
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.event_add(resource, resource_version);
                    }
                }
            }
            "EdgionTls" => {
                if let Ok(resource) = serde_json::from_str::<EdgionTls>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        let cache = self
                            .edgion_tls
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.event_add(resource, resource_version);
                    }
                }
            }
            "Secret" => {
                if let Ok(resource) = serde_json::from_str::<Secret>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        let cache = self
                            .secrets
                            .entry(key)
                            .or_insert_with(|| WatcherCache::new(1000));
                        cache.event_add(resource, resource_version);
                    }
                }
            }
            _ => {}
        }
    }

    fn event_update(&mut self, resource_type: &str, data: String, resource_version: Option<u64>) {
        match resource_type {
            "GatewayClass" => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    if let Some(key) = resource.metadata.name.clone() {
                        if let Some(cache) = self.gateway_classes.get_mut(&key) {
                            cache.event_update(resource, resource_version);
                        }
                    }
                }
            }
            "GatewayClassSpec" => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    let key = "default".to_string();
                    if let Some(cache) = self.gateway_class_specs.get_mut(&key) {
                        cache.event_update(resource, resource_version);
                    }
                }
            }
            "Gateway" => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    let key = resource.spec.gateway_class_name.clone();
                    if let Some(cache) = self.gateways.get_mut(&key) {
                        cache.event_update(resource, resource_version);
                    }
                }
            }
            "HTTPRoute" => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    if let Some(key) = resource
                        .spec
                        .parent_refs
                        .as_ref()
                        .and_then(|refs| refs.first())
                        .map(|parent| parent.name.clone())
                    {
                        if let Some(cache) = self.routes.get_mut(&key) {
                            cache.event_update(resource, resource_version);
                        }
                    }
                }
            }
            "Service" => {
                if let Ok(resource) = serde_json::from_str::<Service>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        if let Some(cache) = self.services.get_mut(&key) {
                            cache.event_update(resource, resource_version);
                        }
                    }
                }
            }
            "EndpointSlice" => {
                if let Ok(resource) = serde_json::from_str::<EndpointSlice>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        if let Some(cache) = self.endpoint_slices.get_mut(&key) {
                            cache.event_update(resource, resource_version);
                        }
                    }
                }
            }
            "EdgionTls" => {
                if let Ok(resource) = serde_json::from_str::<EdgionTls>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        if let Some(cache) = self.edgion_tls.get_mut(&key) {
                            cache.event_update(resource, resource_version);
                        }
                    }
                }
            }
            "Secret" => {
                if let Ok(resource) = serde_json::from_str::<Secret>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        if let Some(cache) = self.secrets.get_mut(&key) {
                            cache.event_update(resource, resource_version);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn event_del(&mut self, resource_type: &str, data: String, resource_version: Option<u64>) {
        match resource_type {
            "GatewayClass" => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    if let Some(key) = resource.metadata.name.clone() {
                        if let Some(cache) = self.gateway_classes.get_mut(&key) {
                            cache.event_del(resource, resource_version);
                        }
                    }
                }
            }
            "GatewayClassSpec" => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    let key = "default".to_string();
                    if let Some(cache) = self.gateway_class_specs.get_mut(&key) {
                        cache.event_del(resource, resource_version);
                    }
                }
            }
            "Gateway" => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    let key = resource.spec.gateway_class_name.clone();
                    if let Some(cache) = self.gateways.get_mut(&key) {
                        cache.event_del(resource, resource_version);
                    }
                }
            }
            "HTTPRoute" => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    if let Some(key) = resource
                        .spec
                        .parent_refs
                        .as_ref()
                        .and_then(|refs| refs.first())
                        .map(|parent| parent.name.clone())
                    {
                        if let Some(cache) = self.routes.get_mut(&key) {
                            cache.event_del(resource, resource_version);
                        }
                    }
                }
            }
            "Service" => {
                if let Ok(resource) = serde_json::from_str::<Service>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        if let Some(cache) = self.services.get_mut(&key) {
                            cache.event_del(resource, resource_version);
                        }
                    }
                }
            }
            "EndpointSlice" => {
                if let Ok(resource) = serde_json::from_str::<EndpointSlice>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        if let Some(cache) = self.endpoint_slices.get_mut(&key) {
                            cache.event_del(resource, resource_version);
                        }
                    }
                }
            }
            "EdgionTls" => {
                if let Ok(resource) = serde_json::from_str::<EdgionTls>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        if let Some(cache) = self.edgion_tls.get_mut(&key) {
                            cache.event_del(resource, resource_version);
                        }
                    }
                }
            }
            "Secret" => {
                if let Ok(resource) = serde_json::from_str::<Secret>(&data) {
                    if let Some(key) = resource.metadata.namespace.as_ref().and_then(|ns| {
                        resource
                            .metadata
                            .name
                            .as_ref()
                            .map(|name| format!("{}/{}", ns, name))
                    }) {
                        if let Some(cache) = self.secrets.get_mut(&key) {
                            cache.event_del(resource, resource_version);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
