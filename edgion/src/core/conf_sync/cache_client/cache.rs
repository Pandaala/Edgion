use crate::core::conf_sync::cache_server::{EventDispatch, ListData, Versionable};
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::{ResourceMeta, WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED};
use kube::{Resource, ResourceExt};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::RwLock as AsyncRwLock;
use tonic::transport::Channel;

/// Internal cache data structure combining data and version under single lock
struct CacheData<T> {
    data: HashMap<String, T>,
    resource_version: u64,
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
}

impl<T: Versionable + Resource> ClientCache<T> {
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
        }
    }

    /// Set the gRPC client for this cache
    pub async fn set_grpc_client(&self, client: ConfigSyncClientService<Channel>) {
        let mut guard = self.grpc_client.write().await;
        *guard = Some(client);
    }

    /// Get current resource version
    pub fn get_resource_version(&self) -> u64 {
        self.cache_data.read().unwrap().resource_version
    }

    /// Set current resource version
    pub fn set_resource_version(&self, version: u64) {
        self.cache_data.write().unwrap().resource_version = version;
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

impl<T: Versionable + Resource + Clone + Send + 'static> EventDispatch<T> for ClientCache<T> {
    fn apply_change(&self, change: ResourceChange, resource: T)
    where
        T: Send + 'static,
    {
        let version = resource.get_version();
        if resource.get_version() == 0 {
            tracing::warn!(
                component = "cache_client",
                event = "apply_change",
                change = ?change,
                kind = std::any::type_name::<T>(),
                name = ?resource.name_any(),
                namespace = ?resource.namespace(),
                version = 0,
                "Applying change to cache with version 0"
            );
        } else {
            tracing::info!(
                component = "cache_client",
                event = "apply_change",
                change = ?change,
                kind = std::any::type_name::<T>(),
                name = ?resource.name_any(),
                namespace = ?resource.namespace(),
                version = resource.get_version(),
                "Applying change to cache"
            );
        }

        let mut cache = self.cache_data.write().unwrap();
        match change {
            ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                cache.data.insert(version.to_string(), resource);
                if version > cache.resource_version {
                    cache.resource_version = version;
                }
            }
            ResourceChange::EventDelete => {
                cache.data.remove(&version.to_string());
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
    T: Versionable + Resource + ResourceMeta + Clone + Send + 'static,
{
    /// Sync resources from gRPC server
    pub async fn sync(&self) -> Result<(), tonic::Status> {
        let mut client_guard = self.grpc_client.write().await;
        let client = client_guard
            .as_mut()
            .ok_or_else(|| tonic::Status::internal("gRPC client not initialized"))?;

        let request = tonic::Request::new(crate::core::conf_sync::proto::ListRequest {
            key: self.gateway_class_key.as_ref().clone(),
            kind: T::resource_kind() as i32,
        });

        let response = client.list(request).await?;
        let list_response = response.into_inner();

        tracing::info!(
            kind = T::kind_name(),
            bytes = list_response.data.len(),
            version = list_response.resource_version,
            "Syncing resource"
        );

        // Parse JSON array directly to concrete type
        let resources: Vec<T> = serde_json::from_str(&list_response.data).map_err(|e| {
            tracing::error!(
                kind = T::kind_name(),
                error = %e,
                "Failed to parse list response"
            );
            tonic::Status::internal(format!("Failed to parse list response: {}", e))
        })?;

        tracing::info!(kind = T::kind_name(), count = resources.len(), "Parsed resources");

        // Apply to cache
        for resource in resources {
            self.apply_change(ResourceChange::InitAdd, resource);
        }

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

            loop {
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
                            tracing::warn!(
                                kind = T::kind_name(),
                                client_id = %client_id,
                                error = %e,
                                from_version = from_version,
                                "Watch version error, performing re-list"
                            );

                            // Perform list to get latest data
                            let list_request = tonic::Request::new(crate::core::conf_sync::proto::ListRequest {
                                key: gateway_class_key.as_ref().clone(),
                                kind: T::resource_kind() as i32,
                            });

                            match client.list(list_request).await {
                                Ok(list_response) => {
                                    let list_data = list_response.into_inner();

                                    match serde_json::from_str::<Vec<T>>(&list_data.data) {
                                        Ok(resources) => {
                                            // Clear and update cache with fresh data under single lock
                                            let mut cache = cache_data.write().unwrap();
                                            cache.data.clear();
                                            for resource in resources {
                                                let version = resource.get_version();
                                                cache.data.insert(version.to_string(), resource);
                                            }

                                            // Update resource version
                                            from_version = list_data.resource_version;
                                            cache.resource_version = from_version;

                                            tracing::info!(
                                                kind = T::kind_name(),
                                                count = cache.data.len(),
                                                new_version = from_version,
                                                "Re-list completed successfully"
                                            );
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                kind = T::kind_name(),
                                                error = %e,
                                                "Failed to parse list response after version error"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        kind = T::kind_name(),
                                        error = %e,
                                        "Failed to perform re-list after version error"
                                    );
                                }
                            }
                        } else {
                            tracing::error!(
                                kind = T::kind_name(),
                                client_id = %client_id,
                                error = %e,
                                "Failed to start watch"
                            );
                        }

                        drop(client_guard);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                drop(client_guard);

                tracing::info!(
                    kind = T::kind_name(),
                    client_id = %client_id,
                    from_version = from_version,
                    "Watch started"
                );

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
                                                let mut cache = cache_data.write().unwrap();
                                                match change {
                                                    ResourceChange::InitAdd
                                                    | ResourceChange::EventAdd
                                                    | ResourceChange::EventUpdate => {
                                                        cache.data.insert(version.to_string(), resource);
                                                        if version > cache.resource_version {
                                                            cache.resource_version = version;
                                                        }
                                                    }
                                                    ResourceChange::EventDelete => {
                                                        cache.data.remove(&version.to_string());
                                                        if version > cache.resource_version {
                                                            cache.resource_version = version;
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                tracing::error!(
                                                    error = %e,
                                                    kind = T::kind_name(),
                                                    "Failed to parse resource from watch event"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            tracing::info!(
                                kind = T::kind_name(),
                                client_id = %client_id,
                                "Watch stream ended, reconnecting"
                            );
                            break;
                        }
                        Err(e) => {
                            let error_message = e.message();

                            // Check if stream error is version-related
                            if error_message.contains(WATCH_ERR_VERSION_UNEXPECTED)
                                || error_message.contains(WATCH_ERR_TOO_OLD_VERSION)
                            {
                                tracing::warn!(
                                    kind = T::kind_name(),
                                    client_id = %client_id,
                                    error = %e,
                                    "Watch stream version error, will re-list on next iteration"
                                );
                            } else {
                                tracing::error!(
                                    kind = T::kind_name(),
                                    client_id = %client_id,
                                    error = %e,
                                    "Watch stream error, reconnecting"
                                );
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
