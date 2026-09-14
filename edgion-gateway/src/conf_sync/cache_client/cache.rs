use super::cache_data::CacheData;
use edgion_common::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use edgion_common::conf_sync::types::ListData;
use edgion_resources::ResourceMeta;
use kube::Resource;
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::RwLock as AsyncRwLock;
use tonic::transport::Channel;

/// Type-erased trait for ClientCache operations that don't depend on T.
///
/// This allows ConfigClient to provide a unified dispatch method via ResourceKind,
/// eliminating the need for hand-maintained string match arms in multiple places.
/// When a new ResourceKind variant is added, the compiler will enforce exhaustiveness
/// at the dispatch site (ConfigClient::get_dyn_cache), preventing silent omissions.
#[async_trait::async_trait]
pub trait DynClientCache: Send + Sync {
    /// Check if this cache is ready (has completed first list)
    fn is_ready(&self) -> bool;

    /// Mark this cache as ready (used when watch is skipped)
    fn set_ready(&self);

    /// Set the gRPC client for this cache
    async fn set_grpc_client_dyn(&self, client: ConfigSyncClientService<Channel>);

    /// Start watching resources from gRPC server
    async fn start_watch_dyn(&self) -> Result<(), tonic::Status>;

    /// Human-readable kind name (e.g., "HTTPRoute"). Forwarded from `T::kind_name()`,
    /// so it stays in lockstep with the resource's `ResourceMeta` impl.
    fn kind_name_dyn(&self) -> &'static str;

    /// Number of entries currently held by this cache.
    fn len_dyn(&self) -> usize;

    /// Snapshot of all keys currently held by this cache.
    fn keys_dyn(&self) -> Vec<String>;

    /// Serialize all cached resources of this kind to JSON.
    ///
    /// Secret redaction for admin output is applied by the field-level serde adapters
    /// (`types::redact`) when the caller holds an `AdminRedactGuard`; this method needs
    /// no special-casing. INVARIANT: keep this serialization synchronous and on the
    /// caller's thread (the guard is a thread-local).
    fn list_json_dyn(&self) -> Vec<serde_json::Value>;
}

pub struct ClientCache<T>
where
    T: kube::Resource,
{
    // data and version protected by single lock
    pub(crate) cache_data: Arc<RwLock<CacheData<T>>>,

    // gRPC conf_client (optional, for sync/watch)
    pub(crate) grpc_client: Arc<AsyncRwLock<Option<ConfigSyncClientService<Channel>>>>,

    // conf_client identification
    pub(crate) client_id: Arc<str>,
    pub(crate) client_name: Arc<str>,
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
            client_id: Arc::from(client_id),
            client_name: Arc::from(client_name),
        }
    }

    pub fn is_ready(&self) -> bool {
        let cache = self.cache_data.read();
        cache.is_ready()
    }

    /// Set the gRPC conf_client for this cache
    pub async fn set_grpc_client(&self, client: ConfigSyncClientService<Channel>) {
        let mut guard = self.grpc_client.write().await;
        *guard = Some(client);
    }

    pub fn set_conf_processor(&self, processor: Box<dyn edgion_common::conf_sync::traits::ConfHandler<T> + Send + Sync>)
    where
        T: Clone + ResourceMeta,
    {
        let mut cache = self.cache_data.write();
        cache.set_conf_processor(processor, self.cache_data.clone());
    }

    /// Replace all cached resources; keys are `resource.key_name()` (namespace/name).
    /// Test-only: production resets go through `CacheData::reset` directly
    /// (see `event_dispatch.rs`), not this wrapper.
    #[cfg(test)]
    pub fn reset(&self, resources: Vec<T>, sync_version: u64)
    where
        T: ResourceMeta,
    {
        let mut cache = self.cache_data.write();
        cache.reset(resources, sync_version);
    }

    /// List all data as owned values (cloned from internal Arc storage)
    pub fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let cache = self.cache_data.read();
        let data = cache.values().map(|arc| (**arc).clone()).collect();
        let sync_version = cache.sync_version();
        ListData::new(data, sync_version)
    }

    pub fn get(&self, key: &str) -> Option<T>
    where
        T: Clone,
    {
        let cache = self.cache_data.read();
        cache.get(key).map(|arc| (**arc).clone())
    }

    pub fn keys(&self) -> Vec<String> {
        let cache = self.cache_data.read();
        cache.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        let cache = self.cache_data.read();
        cache.len()
    }

    pub fn is_empty(&self) -> bool {
        let cache = self.cache_data.read();
        cache.is_empty()
    }
}

#[async_trait::async_trait]
impl<T> DynClientCache for ClientCache<T>
where
    T: ResourceMeta + Resource + Clone + Send + Sync + serde::Serialize + 'static,
{
    fn is_ready(&self) -> bool {
        self.is_ready()
    }

    fn set_ready(&self) {
        let mut cache = self.cache_data.write();
        cache.set_ready();
    }

    async fn set_grpc_client_dyn(&self, client: ConfigSyncClientService<Channel>) {
        self.set_grpc_client(client).await;
    }

    async fn start_watch_dyn(&self) -> Result<(), tonic::Status> {
        self.start_watch().await
    }

    fn kind_name_dyn(&self) -> &'static str {
        T::kind_name()
    }

    fn len_dyn(&self) -> usize {
        self.len()
    }

    fn keys_dyn(&self) -> Vec<String> {
        self.keys()
    }

    fn list_json_dyn(&self) -> Vec<serde_json::Value> {
        self.list_owned()
            .data
            .into_iter()
            .filter_map(|item| serde_json::to_value(item).ok())
            .collect()
    }
}
