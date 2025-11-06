use std::collections::HashMap;
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use crate::core::conf_sync::watcher_cache::WatcherCache;
use crate::types::{EdgionTls, Gateway, GatewayClassSpec, HTTPRoute};

pub type GatewayClass = String;

pub struct WatcherMgr {
    gateway_classes: HashMap<GatewayClass, WatcherCache<GatewayClass>>,
    gateways: HashMap<String, WatcherCache<Gateway>>,
    routes: HashMap<GatewayClass, WatcherCache<HTTPRoute>>,
    services: HashMap<GatewayClass, WatcherCache<Service>>,
    endpoint_slices: HashMap<GatewayClass, WatcherCache<EndpointSlice>>,
    edgion_tls: HashMap<GatewayClass, WatcherCache<EdgionTls>>,
    secrets: HashMap<GatewayClass, WatcherCache<Secret>>,
}
