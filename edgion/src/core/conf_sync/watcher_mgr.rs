use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::core::conf_sync::watcher_cache::{ListData, WatchResponse, WatcherCache};
use crate::types::{EdgionTls, Gateway, GatewayClassSpec, HTTPRoute};

pub type GatewayClass = String;

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
    gateway_classes: HashMap<GatewayClass, WatcherCache<GatewayClass>>,
    gateways: HashMap<String, WatcherCache<Gateway>>,
    routes: HashMap<GatewayClass, WatcherCache<HTTPRoute>>,
    services: HashMap<GatewayClass, WatcherCache<Service>>,
    endpoint_slices: HashMap<GatewayClass, WatcherCache<EndpointSlice>>,
    edgion_tls: HashMap<GatewayClass, WatcherCache<EdgionTls>>,
    secrets: HashMap<GatewayClass, WatcherCache<Secret>>,
}

impl WatcherMgr {
    pub fn new() -> Self {
        Self {
            gateway_classes: HashMap::new(),
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
