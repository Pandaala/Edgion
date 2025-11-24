use crate::core::conf_sync::cache_client::ConfProcessor;
use crate::core::conf_sync::cache_server::{EventDispatch, ListData};
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::{ResourceMeta, WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED};
use kube::{Resource, ResourceExt};
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
struct CacheData<T> {
    data: HashMap<String, T>,
    resource_version: u64,
}

impl<T: ResourceMeta> CacheData<T> {
    /// Reset cache with a complete set of resources
    /// Uses resource.key_name() (namespace/name) as the key for each resource
    fn reset(&mut self, resources: Vec<T>, resource_version: u64) {
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
    // data and version protected by single lock
    cache_data: Arc<RwLock<CacheData<T>>>,

    // gRPC client (optional, for sync/watch)
    grpc_client: Arc<AsyncRwLock<Option<ConfigSyncClientService<Channel>>>>,

    // gateway class key
    gateway_class_key: Arc<String>,

    // client identification
    client_id: Arc<String>,
    client_name: Arc<String>,

    // configuration processor (optional)
    conf_processor: Arc<RwLock<Option<Box<dyn ConfProcessor<T> + Send + Sync>>>>,

    // compressed events storage
    compress_events: Arc<RwLock<CompressEvent>>,
}

impl<T: ResourceMeta + Resource> ClientCache<T> {
    pub fn new(gateway_class_key: String, client_id: String, client_name: String) -> Self {
        Self {
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

impl<T: ResourceMeta + Resource + Clone + Send + 'static> EventDispatch<T> for ClientCache<T> {
    fn apply_change(&self, change: ResourceChange, resource: T)
    where
        T: Send + 'static,
    {
        // Extract resource key and add event to compress_events
        let key = resource.key_name();
        {
            let mut compress_events = self.compress_events.write().unwrap();
            compress_events.add_event(key, change);
        }

        let version = resource.get_version();
        if resource.get_version() == 0 {
            tracing::warn!(component = "cache_client", event = "apply_change", change = ?change, kind = std::any::type_name::<T>(), name = ?resource.name_any(), namespace = ?resource.namespace(), version = 0, "Applying change to cache with version 0");
        } else {
            tracing::info!(component = "cache_client", event = "apply_change", change = ?change, kind = std::any::type_name::<T>(), name = ?resource.name_any(), namespace = ?resource.namespace(), version = resource.get_version(), "Applying change to cache");
        }

        let mut cache = self.cache_data.write().unwrap();
        let resource_key = resource.key_name(); // Use namespace/name as key
        match change {
            ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                cache.data.insert(resource_key, resource);
                if version > cache.resource_version {
                    cache.resource_version = version;
                }
            }
            ResourceChange::EventDelete => {
                cache.data.remove(&resource_key);
                if version > cache.resource_version {
                    cache.resource_version = version;
                }
            }
        }
    }

    fn set_ready(&self) {
        // HubCache doesn't need ready state, but we keep the method for trait compatibility
    }
}

// Additional methods for ResourceMeta types
impl<T> ClientCache<T>
where
    T: ResourceMeta + Resource + Clone + Send + 'static,
{
    /// Internal helper function to perform list and reset cache
    /// Returns the resource version on success
    async fn list_and_reset(
        client: &mut ConfigSyncClientService<Channel>,
        gateway_class_key: &str,
        cache_data: &Arc<RwLock<CacheData<T>>>,
        log_context: &str,
    ) -> Result<u64, tonic::Status> {
        let list_request = tonic::Request::new(crate::core::conf_sync::proto::ListRequest {
            key: gateway_class_key.to_string(),
            kind: T::resource_kind() as i32,
        });

        let response = client.list(list_request).await?;
        let list_data = response.into_inner();

        tracing::info!(kind = T::kind_name(), bytes = list_data.data.len(), version = list_data.resource_version, context = log_context, "Listing resources");

        // Parse JSON array directly to concrete type
        let resources: Vec<T> = serde_json::from_str(&list_data.data).map_err(|e| {
            tracing::error!(kind = T::kind_name(), error = %e, context = log_context, "Failed to parse list response");
            tonic::Status::internal(format!("Failed to parse list response: {}", e))
        })?;

        tracing::info!(kind = T::kind_name(), count = resources.len(), context = log_context, "Parsed resources");

        // Use reset method to rebuild cache with fresh data
        {
            let mut cache = cache_data.write().unwrap();
            cache.reset(resources, list_data.resource_version);
        }

        Ok(list_data.resource_version)
    }

    /// Sync resources from gRPC server
    pub async fn sync(&self) -> Result<(), tonic::Status> {
        let mut client_guard = self.grpc_client.write().await;
        let client = client_guard
            .as_mut()
            .ok_or_else(|| tonic::Status::internal("gRPC client not initialized"))?;

        Self::list_and_reset(
            client,
            self.gateway_class_key.as_ref(),
            &self.cache_data,
            "sync",
        )
        .await?;

        tracing::info!(kind = T::kind_name(), "Finished syncing");
        Ok(())
    }

    /// Start watching resources from gRPC server
    pub async fn start_watch(&self) -> Result<(), tonic::Status> {
        let grpc_client = self.grpc_client.clone();
        let cache_data = self.cache_data.clone();
        let gateway_class_key = self.gateway_class_key.clone();
        let client_id = self.client_id.clone();
        let client_name = self.client_name.clone();

        tokio::spawn(async move {
            let mut from_version = cache_data.read().unwrap().resource_version;
            let mut is_first_watch = true;  // Flag to distinguish first watch from reconnection

            loop {
                // On reconnection (not first watch), perform relist to sync with server
                if !is_first_watch {
                    tracing::info!(kind = T::kind_name(), client_id = %client_id, "Reconnecting - performing relist before watch");

                    let mut client_guard = grpc_client.write().await;
                    if let Some(client) = client_guard.as_mut() {
                        match Self::list_and_reset(
                            client,
                            gateway_class_key.as_ref(),
                            &cache_data,
                            "reconnection relist",
                        )
                        .await
                        {
                            Ok(resource_version) => {
                                // Update resource version
                                from_version = resource_version;
                                let count = {
                                    let cache = cache_data.read().unwrap();
                                    cache.data.len()
                                };
                                tracing::info!(kind = T::kind_name(), count = count, new_version = from_version, "Reconnection relist completed successfully");
                            }
                            Err(e) => {
                                tracing::error!(kind = T::kind_name(), error = %e, "Failed to perform relist on reconnection");
                            }
                        }
                    }
                    drop(client_guard);
                }

                let mut client_guard = grpc_client.write().await;
                let client = match client_guard.as_mut() {
                    Some(c) => c,
                    None => {
                        tracing::error!(kind = T::kind_name(), "gRPC client not initialized for watch");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                let request = tonic::Request::new(crate::core::conf_sync::proto::WatchRequest {
                    key: gateway_class_key.as_ref().clone(),
                    kind: T::resource_kind() as i32,
                    client_id: client_id.as_ref().clone(),
                    client_name: client_name.as_ref().clone(),
                    from_version,
                });

                let mut stream = match client.watch(request).await {
                    Ok(response) => response.into_inner(),
                    Err(e) => {
                        let error_message = e.message();

                        // Check if error is version-related, need to re-list
                        if error_message.contains(WATCH_ERR_VERSION_UNEXPECTED)
                            || error_message.contains(WATCH_ERR_TOO_OLD_VERSION)
                        {
                            tracing::warn!(kind = T::kind_name(), client_id = %client_id, error = %e, from_version = from_version, "Watch version error, performing re-list");

                            // Perform list to get latest data
                            match Self::list_and_reset(
                                client,
                                gateway_class_key.as_ref(),
                                &cache_data,
                                "version error relist",
                            )
                            .await
                            {
                                Ok(resource_version) => {
                                    // Update resource version
                                    from_version = resource_version;
                                    let count = {
                                        let cache = cache_data.read().unwrap();
                                        cache.data.len()
                                    };
                                    tracing::info!(kind = T::kind_name(), count = count, new_version = from_version, "Re-list completed successfully");
                                }
                                Err(e) => {
                                    tracing::error!(kind = T::kind_name(), error = %e, "Failed to perform re-list after version error");
                                }
                            }
                        } else {
                            tracing::error!(kind = T::kind_name(), client_id = %client_id, error = %e, "Failed to start watch");
                        }

                        drop(client_guard);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                drop(client_guard);

                if is_first_watch {
                    tracing::info!(kind = T::kind_name(), client_id = %client_id, from_version = from_version, "Watch started (first time)");
                    is_first_watch = false;  // Mark that first watch has been done
                } else {
                    tracing::info!(kind = T::kind_name(), client_id = %client_id, from_version = from_version, "Watch restarted after reconnection");
                }

                // Process stream messages
                loop {
                    match stream.message().await {
                        Ok(Some(watch_response)) => {
                            from_version = watch_response.resource_version;

                            let events: Vec<serde_json::Value> = match serde_json::from_str(&watch_response.data) {
                                Ok(events) => events,
                                Err(e) => {
                                    tracing::error!(error = %e, "Failed to parse watch response events");
                                    continue;
                                }
                            };

                            for event in events {
                                if let Some(event_type) = event.get("type").and_then(|v| v.as_str()) {
                                    if let Some(event_data) = event.get("data") {
                                        match serde_json::from_value::<T>(event_data.clone()) {
                                            Ok(resource) => {
                                                let change = match event_type {
                                                    "add" => ResourceChange::EventAdd,
                                                    "update" => ResourceChange::EventUpdate,
                                                    "delete" => ResourceChange::EventDelete,
                                                    _ => continue,
                                                };

                                                // Apply change using single lock
                                                let version = resource.get_version();
                                                let resource_key = resource.key_name(); // Use namespace/name as key
                                                let mut cache = cache_data.write().unwrap();
                                                match change {
                                                    ResourceChange::InitAdd
                                                    | ResourceChange::EventAdd
                                                    | ResourceChange::EventUpdate => {
                                                        cache.data.insert(resource_key, resource);
                                                        if version > cache.resource_version {
                                                            cache.resource_version = version;
                                                        }
                                                    }
                                                    ResourceChange::EventDelete => {
                                                        cache.data.remove(&resource_key);
                                                        if version > cache.resource_version {
                                                            cache.resource_version = version;
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                tracing::error!(error = %e, kind = T::kind_name(), "Failed to parse resource from watch event");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            tracing::info!(kind = T::kind_name(), client_id = %client_id, "Watch stream ended, reconnecting");
                            break;
                        }
                        Err(e) => {
                            let error_message = e.message();

                            // Check if stream error is version-related
                            if error_message.contains(WATCH_ERR_VERSION_UNEXPECTED)
                                || error_message.contains(WATCH_ERR_TOO_OLD_VERSION)
                            {
                                tracing::warn!(kind = T::kind_name(), client_id = %client_id, error = %e, "Watch stream version error, will re-list on next iteration");
                            } else {
                                tracing::error!(kind = T::kind_name(), client_id = %client_id, error = %e, "Watch stream error, reconnecting");
                            }
                            break;
                        }
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

        Ok(())
    }
}
