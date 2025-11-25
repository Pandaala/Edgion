use crate::core::conf_sync::cache_client::ConfProcessor;
use crate::core::conf_sync::cache_server::ListData;
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::ResourceMeta;
use kube::Resource;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::RwLock as AsyncRwLock;
use tonic::transport::Channel;

/// Compressed event storage: key is resource key (namespace/name), value is list of events
pub struct CompressEvent {
    events: HashMap<String, Vec<ResourceChange>>,
}

impl CompressEvent {
    pub fn new() -> Self {
        Self {
            events: HashMap::new(),
        }
    }

    /// Add an event for a resource key
    pub fn add_event(&mut self, key: String, change: ResourceChange) {
        self.events.entry(key).or_insert_with(Vec::new).push(change);
    }

    /// Get events for a resource key
    pub fn get_events(&self, key: &str) -> Option<&Vec<ResourceChange>> {
        self.events.get(key)
    }

    /// Clear all events
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// Internal cache data structure combining data and version under single lock
pub(crate) struct CacheData<T> {
    pub(crate) data: HashMap<String, T>,
    pub(crate) resource_version: u64,
}

impl<T: ResourceMeta> CacheData<T> {
    /// Reset cache with a complete set of resources
    /// Uses resource.key_name() (namespace/name) as the key for each resource
    pub(crate) fn reset(&mut self, resources: Vec<T>, resource_version: u64) {
        self.data.clear();
        for resource in resources {
            let key = resource.key_name();
            self.data.insert(key, resource);
        }
        self.resource_version = resource_version;
    }
}

pub struct ClientCache<T>
where
    T: kube::Resource,
{
    // wait for init complete
    pub(crate) ready: RwLock<bool>,

    // data and version protected by single lock
    pub(crate) cache_data: Arc<RwLock<CacheData<T>>>,

    // gRPC client (optional, for sync/watch)
    pub(crate) grpc_client: Arc<AsyncRwLock<Option<ConfigSyncClientService<Channel>>>>,

    // gateway class key
    pub(crate) gateway_class_key: Arc<String>,

    // client identification
    pub(crate) client_id: Arc<String>,
    pub(crate) client_name: Arc<String>,


    // todo 每次重新list后，需要清空events，同时重新full rebuild conf_processor
    //      定时compress events, 触发update rebuild
    // configuration processor (optional)
    conf_processor: Arc<RwLock<Option<Box<dyn ConfProcessor<T> + Send + Sync>>>>,

    // compressed events storage
    pub(crate) compress_events: Arc<RwLock<CompressEvent>>,
}

impl<T: kube::Resource> Clone for ClientCache<T> {
    fn clone(&self) -> Self {
        Self {
            ready: RwLock::new(*self.ready.read().unwrap()),
            cache_data: self.cache_data.clone(),
            grpc_client: self.grpc_client.clone(),
            gateway_class_key: self.gateway_class_key.clone(),
            client_id: self.client_id.clone(),
            client_name: self.client_name.clone(),
            conf_processor: self.conf_processor.clone(),
            compress_events: self.compress_events.clone(),
        }
    }
}

impl<T: ResourceMeta + Resource> ClientCache<T> {
    pub fn new(gateway_class_key: String, client_id: String, client_name: String) -> Self {
        Self {
            ready: RwLock::new(false),
            cache_data: Arc::new(RwLock::new(CacheData {
                data: HashMap::new(),
                resource_version: 0,
            })),
            grpc_client: Arc::new(AsyncRwLock::new(None)),
            gateway_class_key: Arc::new(gateway_class_key),
            client_id: Arc::new(client_id),
            client_name: Arc::new(client_name),
            conf_processor: Arc::new(RwLock::new(None)),
            compress_events: Arc::new(RwLock::new(CompressEvent::new())),
        }
    }

    /// Check if cache is ready
    pub fn is_ready(&self) -> bool {
        *self.ready.read().unwrap()
    }

    /// Set the gRPC client for this cache
    pub async fn set_grpc_client(&self, client: ConfigSyncClientService<Channel>) {
        let mut guard = self.grpc_client.write().await;
        *guard = Some(client);
    }

    /// Set the configuration processor for this cache
    pub fn set_conf_processor(&self, processor: Box<dyn ConfProcessor<T> + Send + Sync>) {
        let mut guard = self.conf_processor.write().unwrap();
        *guard = Some(processor);
    }

    /// Get current resource version
    pub fn get_resource_version(&self) -> u64 {
        self.cache_data.read().unwrap().resource_version
    }

    /// Set current resource version
    pub fn set_resource_version(&self, version: u64) {
        self.cache_data.write().unwrap().resource_version = version;
    }

    /// Reset cache with a complete set of resources
    /// This clears existing cache and rebuilds it with the provided resources
    /// Uses resource.key_name() (namespace/name) as the key for each resource
    pub fn reset(&self, resources: Vec<T>, resource_version: u64)
    where
        T: ResourceMeta,
    {
        let mut cache = self.cache_data.write().unwrap();
        cache.reset(resources, resource_version);
    }

    /// List all data - returns all resources in the cache with resource version
    pub fn list(&self) -> ListData<T>
    where
        T: Clone,
    {
        self.list_owned()
    }

    /// List all data as owned values (cloned)
    pub fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let cache = self.cache_data.read().unwrap();
        let data = cache.data.values().cloned().collect();
        let resource_version = cache.resource_version;
        ListData::new(data, resource_version)
    }

    /// Get a resource by key
    pub fn get(&self, key: &str) -> Option<T>
    where
        T: Clone,
    {
        let cache = self.cache_data.read().unwrap();
        cache.data.get(key).cloned()
    }

    /// Get all keys
    pub fn keys(&self) -> Vec<String> {
        let cache = self.cache_data.read().unwrap();
        cache.data.keys().cloned().collect()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        let cache = self.cache_data.read().unwrap();
        cache.data.is_empty()
    }

    /// Get the number of resources in the cache
    pub fn len(&self) -> usize {
        let cache = self.cache_data.read().unwrap();
        cache.data.len()
    }
}
