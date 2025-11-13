use crate::core::conf_load::SharedDispatcher;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{
    ConfigServer, EventDispatch, EventDispatcher, ServerCache, Versionable,
};
use crate::types::{GatewayClass, ResourceKind};
use serde_yaml::from_str;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime::Handle;

const CACHE_CAPACITY: u32 = 1024;
pub(crate) const DEFAULT_GATEWAY_CLASS_KEY: &str = "default";

pub(crate) struct ConfigServerBridge {
    server: Arc<tokio::sync::Mutex<ConfigServer>>,
    dispatcher: SharedDispatcher,
}

impl ConfigServerBridge {
    pub fn new() -> Self {
        let server = Arc::new(tokio::sync::Mutex::new(ConfigServer::new()));
        let dispatcher_impl = ConfigServerDispatcher {
            server: server.clone(),
        };
        let dispatcher: SharedDispatcher = Arc::new(tokio::sync::Mutex::new(Box::new(
            dispatcher_impl,
        )
            as Box<dyn EventDispatcher>));

        Self { server, dispatcher }
    }

    pub fn dispatcher(&self) -> SharedDispatcher {
        self.dispatcher.clone()
    }

    pub fn server(&self) -> Arc<tokio::sync::Mutex<ConfigServer>> {
        self.server.clone()
    }

    pub async fn ensure_gateway_class(&self, key: &str) {
        let mut guard = self.server.lock().await;
        ensure_all_caches_for_key(&mut guard, key);
    }

    pub async fn ensure_default_gateway_class(&self) {
        self.ensure_gateway_class(DEFAULT_GATEWAY_CLASS_KEY).await;
    }
}

struct ConfigServerDispatcher {
    server: Arc<tokio::sync::Mutex<ConfigServer>>,
}

impl ConfigServerDispatcher {
    fn with_server<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut ConfigServer) -> R,
    {
        tokio::task::block_in_place(|| {
            Handle::current().block_on(async {
                let mut guard = self.server.lock().await;
                f(&mut guard)
            })
        })
    }
}

impl EventDispatcher for ConfigServerDispatcher {
    fn apply_resource_change(
        &mut self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        let detected = resource_type.or_else(|| ResourceKind::from_content(&data));
        self.with_server(|server| {
            if let Some(ResourceKind::GatewayClass) = detected {
                ensure_gateway_class_from_content(server, &data);
            }
            server.apply_resource_change(change, detected, data, resource_version);
        });
    }

    fn set_ready(&mut self) {
        self.with_server(|server| server.set_ready());
    }
}

fn ensure_gateway_class_from_content(server: &mut ConfigServer, data: &str) {
    if let Ok(resource) = from_str::<GatewayClass>(data) {
        if let Some(name) = resource.metadata.name.clone() {
            ensure_all_caches_for_key(server, &name);
        }
    }
    ensure_all_caches_for_key(server, DEFAULT_GATEWAY_CLASS_KEY);
}

fn ensure_all_caches_for_key(server: &mut ConfigServer, key: &str) {
    ensure_cache(&mut server.gateway_classes, key);
    ensure_cache(&mut server.edgion_gateway_configs, key);
    ensure_cache(&mut server.gateways, key);
    ensure_cache(&mut server.routes, key);
    ensure_cache(&mut server.services, key);
    ensure_cache(&mut server.endpoint_slices, key);
    ensure_cache(&mut server.edgion_tls, key);
    ensure_cache(&mut server.secrets, key);
}

fn ensure_cache<T>(map: &mut HashMap<String, ServerCache<T>>, key: &str)
where
    T: Versionable + Clone + Send + Sync + 'static,
{
    map.entry(key.to_string()).or_insert_with(|| {
        let mut cache = ServerCache::new(CACHE_CAPACITY);
        cache.set_ready();
        cache
    });
}
