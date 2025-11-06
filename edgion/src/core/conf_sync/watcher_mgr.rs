use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::core::conf_sync::traits::EventDispatcher;
use crate::core::conf_sync::watcher_cache::{EventDispatch, ListData, WatchResponse, WatcherCache};
use crate::types::{EdgionTls, Gateway, GatewayClass, GatewayClassSpec, HTTPRoute, ResourceKind};
use anyhow::Result;

pub type GatewayClassKey = String;

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

pub struct ListDataSimple {
    pub data: String,
    pub resource_version: u64,
}

pub struct EventDataSimple {
    pub data: String,
    pub resource_version: u64,
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

    pub fn list(
        &self,
        key: &GatewayClassKey,
        kind: &ResourceKind,
    ) -> Result<ListDataSimple, String> {
        let (data_json, resource_version) = match kind {
            ResourceKind::GatewayClass => {
                let list_data = self
                    .list_gateway_classes(key)
                    .ok_or_else(|| format!("GatewayClass cache not found for key: {}", key))?;
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize GatewayClass data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::GatewayClassSpec => {
                let list_data = self
                    .list_gateway_class_specs(key)
                    .ok_or_else(|| format!("GatewayClassSpec cache not found for key: {}", key))?;
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize GatewayClassSpec data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Gateway => {
                let list_data = self
                    .list_gateways(key)
                    .ok_or_else(|| format!("Gateway cache not found for key: {}", key))?;
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Gateway data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::HTTPRoute => {
                let list_data = self
                    .list_routes(key)
                    .ok_or_else(|| format!("HTTPRoute cache not found for key: {}", key))?;
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize HTTPRoute data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Service => {
                let list_data = self
                    .list_services(key)
                    .ok_or_else(|| format!("Service cache not found for key: {}", key))?;
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Service data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EndpointSlice => {
                let list_data = self
                    .list_endpoint_slices(key)
                    .ok_or_else(|| format!("EndpointSlice cache not found for key: {}", key))?;
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EndpointSlice data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::EdgionTls => {
                let list_data = self
                    .list_edgion_tls(key)
                    .ok_or_else(|| format!("EdgionTls cache not found for key: {}", key))?;
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize EdgionTls data: {}", e))?;
                (json, list_data.resource_version)
            }
            ResourceKind::Secret => {
                let list_data = self
                    .list_secrets(key)
                    .ok_or_else(|| format!("Secret cache not found for key: {}", key))?;
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize Secret data: {}", e))?;
                (json, list_data.resource_version)
            }
        };

        Ok(ListDataSimple {
            data: data_json,
            resource_version,
        })
    }

    pub fn watch(
        &mut self,
        key: &GatewayClassKey,
        kind: &ResourceKind,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Result<mpsc::Receiver<EventDataSimple>, String> {
        let (tx, rx) = mpsc::channel(100);

        match kind {
            ResourceKind::GatewayClass => {
                let mut receiver = self
                    .watch_gateway_classes(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("GatewayClass cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let events_json = match serde_json::to_string(&response.events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize GatewayClass events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version: response.resource_version,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::GatewayClassSpec => {
                let mut receiver = self
                    .watch_gateway_class_specs(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("GatewayClassSpec cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let events_json = match serde_json::to_string(&response.events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize GatewayClassSpec events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version: response.resource_version,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::Gateway => {
                let mut receiver = self
                    .watch_gateways(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("Gateway cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let events_json = match serde_json::to_string(&response.events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize Gateway events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version: response.resource_version,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::HTTPRoute => {
                let mut receiver = self
                    .watch_routes(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("HTTPRoute cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let events_json = match serde_json::to_string(&response.events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize HTTPRoute events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version: response.resource_version,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::Service => {
                let mut receiver = self
                    .watch_services(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("Service cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let events_json = match serde_json::to_string(&response.events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize Service events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version: response.resource_version,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::EndpointSlice => {
                let mut receiver = self
                    .watch_endpoint_slices(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("EndpointSlice cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let events_json = match serde_json::to_string(&response.events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize EndpointSlice events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version: response.resource_version,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::EdgionTls => {
                let mut receiver = self
                    .watch_edgion_tls(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("EdgionTls cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let events_json = match serde_json::to_string(&response.events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize EdgionTls events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version: response.resource_version,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ResourceKind::Secret => {
                let mut receiver = self
                    .watch_secrets(key, client_id, client_name, from_version)
                    .ok_or_else(|| format!("Secret cache not found for key: {}", key))?;
                tokio::spawn(async move {
                    while let Some(response) = receiver.recv().await {
                        let events_json = match serde_json::to_string(&response.events) {
                            Ok(json) => json,
                            Err(e) => {
                                eprintln!("Failed to serialize Secret events: {}", e);
                                continue;
                            }
                        };
                        let event_data = EventDataSimple {
                            data: events_json,
                            resource_version: response.resource_version,
                        };
                        if tx.send(event_data).await.is_err() {
                            break;
                        }
                    }
                });
            }
        }

        Ok(rx)
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
    fn init_add(
        &mut self,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            return;
        };

        match resource_type {
            ResourceKind::GatewayClass => {
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
            ResourceKind::GatewayClassSpec => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    let key = "default".to_string();
                    let cache = self
                        .gateway_class_specs
                        .entry(key)
                        .or_insert_with(|| WatcherCache::new(1000));
                    cache.init_add(resource, resource_version);
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    let key = resource.spec.gateway_class_name.clone();
                    let cache = self
                        .gateways
                        .entry(key)
                        .or_insert_with(|| WatcherCache::new(1000));
                    cache.init_add(resource, resource_version);
                }
            }
            ResourceKind::HTTPRoute => {
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
            ResourceKind::Service => {
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
            ResourceKind::EndpointSlice => {
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
            ResourceKind::EdgionTls => {
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
            ResourceKind::Secret => {
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

    fn event_add(
        &mut self,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            return;
        };

        match resource_type {
            ResourceKind::GatewayClass => {
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
            ResourceKind::GatewayClassSpec => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    let key = "default".to_string();
                    let cache = self
                        .gateway_class_specs
                        .entry(key)
                        .or_insert_with(|| WatcherCache::new(1000));
                    cache.event_add(resource, resource_version);
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    let key = resource.spec.gateway_class_name.clone();
                    let cache = self
                        .gateways
                        .entry(key)
                        .or_insert_with(|| WatcherCache::new(1000));
                    cache.event_add(resource, resource_version);
                }
            }
            ResourceKind::HTTPRoute => {
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
            ResourceKind::Service => {
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
            ResourceKind::EndpointSlice => {
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
            ResourceKind::EdgionTls => {
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
            ResourceKind::Secret => {
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

    fn event_update(
        &mut self,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            return;
        };

        match resource_type {
            ResourceKind::GatewayClass => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    if let Some(key) = resource.metadata.name.clone() {
                        if let Some(cache) = self.gateway_classes.get_mut(&key) {
                            cache.event_update(resource, resource_version);
                        }
                    }
                }
            }
            ResourceKind::GatewayClassSpec => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    let key = "default".to_string();
                    if let Some(cache) = self.gateway_class_specs.get_mut(&key) {
                        cache.event_update(resource, resource_version);
                    }
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    let key = resource.spec.gateway_class_name.clone();
                    if let Some(cache) = self.gateways.get_mut(&key) {
                        cache.event_update(resource, resource_version);
                    }
                }
            }
            ResourceKind::HTTPRoute => {
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
            ResourceKind::Service => {
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
            ResourceKind::EndpointSlice => {
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
            ResourceKind::EdgionTls => {
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
            ResourceKind::Secret => {
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

    fn event_del(
        &mut self,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        let Some(resource_type) = resource_type else {
            return;
        };

        match resource_type {
            ResourceKind::GatewayClass => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    if let Some(key) = resource.metadata.name.clone() {
                        if let Some(cache) = self.gateway_classes.get_mut(&key) {
                            cache.event_del(resource, resource_version);
                        }
                    }
                }
            }
            ResourceKind::GatewayClassSpec => {
                if let Ok(resource) = serde_json::from_str::<GatewayClassSpec>(&data) {
                    let key = "default".to_string();
                    if let Some(cache) = self.gateway_class_specs.get_mut(&key) {
                        cache.event_del(resource, resource_version);
                    }
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    let key = resource.spec.gateway_class_name.clone();
                    if let Some(cache) = self.gateways.get_mut(&key) {
                        cache.event_del(resource, resource_version);
                    }
                }
            }
            ResourceKind::HTTPRoute => {
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
            ResourceKind::Service => {
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
            ResourceKind::EndpointSlice => {
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
            ResourceKind::EdgionTls => {
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
            ResourceKind::Secret => {
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
