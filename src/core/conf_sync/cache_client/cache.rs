use super::cache_data::CacheData;
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::conf_sync::types::ListData;
use crate::types::ResourceMeta;
use kube::Resource;
use std::sync::{Arc, RwLock};
use tokio::sync::RwLock as AsyncRwLock;
use tonic::transport::Channel;

pub struct ClientCache<T>
where
    T: kube::Resource,
{
    // data and version protected by single lock
    pub(crate) cache_data: Arc<RwLock<CacheData<T>>>,

    // gRPC conf_client (optional, for sync/watch)
    pub(crate) grpc_client: Arc<AsyncRwLock<Option<ConfigSyncClientService<Channel>>>>,

    // conf_client identification
    pub(crate) client_id: Arc<String>,
    pub(crate) client_name: Arc<String>,
}

impl<T: kube::Resource> Clone for ClientCache<T> {
    fn clone(&self) -> Self {
        Self {
            cache_data: self.cache_data.clone(),
            grpc_client: self.grpc_client.clone(),
            client_id: self.client_id.clone(),
            client_name: self.client_name.clone(),
        }
    }
}

impl<T: ResourceMeta + Resource> ClientCache<T> {
    pub fn new(client_id: String, client_name: String) -> Self {
        Self {
            cache_data: Arc::new(RwLock::new(CacheData::new())),
            grpc_client: Arc::new(AsyncRwLock::new(None)),
            client_id: Arc::new(client_id),
            client_name: Arc::new(client_name),
        }
    }

    /// Check if cache is ready
    pub fn is_ready(&self) -> bool {
        let cache = self.cache_data.read().unwrap();
        cache.is_ready()
    }

    /// Set the gRPC conf_client for this cache
    pub async fn set_grpc_client(&self, client: ConfigSyncClientService<Channel>) {
        let mut guard = self.grpc_client.write().await;
        *guard = Some(client);
    }

    /// Set the configuration processor for this cache
    pub fn set_conf_processor(&self, processor: Box<dyn crate::core::conf_sync::traits::ConfHandler<T> + Send + Sync>)
    where
        T: Clone + ResourceMeta,
    {
        let mut cache = self.cache_data.write().unwrap();
        cache.set_conf_processor(processor, self.cache_data.clone());
    }

    /// Get current resource version
    pub fn get_resource_version(&self) -> u64 {
        let cache = self.cache_data.read().unwrap();
        cache.resource_version()
    }

    /// Set current resource version
    pub fn set_resource_version(&self, version: u64) {
        let mut cache = self.cache_data.write().unwrap();
        cache.set_resource_version(version);
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
        let data = cache.values().cloned().collect();
        let resource_version = cache.resource_version();
        ListData::new(data, resource_version)
    }

    /// Get a resource by key
    pub fn get(&self, key: &str) -> Option<T>
    where
        T: Clone,
    {
        let cache = self.cache_data.read().unwrap();
        cache.get(key).cloned()
    }

    /// Get all keys
    pub fn keys(&self) -> Vec<String> {
        let cache = self.cache_data.read().unwrap();
        cache.keys().cloned().collect()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        let cache = self.cache_data.read().unwrap();
        cache.is_empty()
    }

    /// Get the number of resources in the cache
    pub fn len(&self) -> usize {
        let cache = self.cache_data.read().unwrap();
        cache.len()
    }

    pub fn trigger_update_event_by_key(&self, key: &str) {
        // Add compress event to trigger partial_update
        let mut cache = self.cache_data.write().unwrap();
        cache.add_compress_event(
            key.to_string(),
            crate::core::conf_sync::traits::ResourceChange::EventUpdate,
        );

        tracing::info!(
            component = "cache_client",
            key = %key,
            "Manually triggered update event for resource"
        );
    }
}
