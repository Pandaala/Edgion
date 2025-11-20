use crate::core::conf_sync::cache_client::GrpcSyncable;
use crate::core::conf_sync::cache_server::{EventDispatch, ListData, Versionable};
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::conf_sync::traits::ResourceChange;
use kube::{Resource, ResourceExt};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::RwLock as AsyncRwLock;
use tonic::transport::Channel;

pub struct ClientCache<T>
where
    T: kube::Resource,
{
    // data
    data: Arc<RwLock<HashMap<String, T>>>,

    // version
    resource_version: Arc<RwLock<u64>>,

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
            data: Arc::new(RwLock::new(HashMap::new())),
            resource_version: Arc::new(RwLock::new(0)),
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
        *self.resource_version.read().unwrap()
    }

    /// Set current resource version
    pub fn set_resource_version(&self, version: u64) {
        *self.resource_version.write().unwrap() = version;
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
        let data = {
            let data_guard = self.data.read().unwrap();
            data_guard.values().cloned().collect()
        };
        let resource_version = *self.resource_version.read().unwrap();
        ListData::new(data, resource_version)
    }

    /// Get a resource by key
    pub fn get(&self, key: &str) -> Option<T>
    where
        T: Clone,
    {
        let data = self.data.read().unwrap();
        data.get(key).cloned()
    }

    /// Get all keys
    pub fn keys(&self) -> Vec<String> {
        let data = self.data.read().unwrap();
        data.keys().cloned().collect()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        let data = self.data.read().unwrap();
        data.is_empty()
    }

    /// Get the number of resources in the cache
    pub fn len(&self) -> usize {
        let data = self.data.read().unwrap();
        data.len()
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

        match change {
            ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                let mut data = self.data.write().unwrap();
                data.insert(version.to_string(), resource);
                let mut resource_version = self.resource_version.write().unwrap();
                if version > *resource_version {
                    *resource_version = version;
                }
            }
            ResourceChange::EventDelete => {
                let mut data = self.data.write().unwrap();
                data.remove(&version.to_string());
                let mut resource_version = self.resource_version.write().unwrap();
                if version > *resource_version {
                    *resource_version = version;
                }
            }
        }
    }

    fn set_ready(&self) {
        // HubCache doesn't need ready state, but we keep the method for trait compatibility
    }
}

// Additional methods for GrpcSyncable types
impl<T> ClientCache<T>
where
    T: Versionable + Resource + GrpcSyncable + Clone + Send + 'static,
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
        let data = self.data.clone();
        let resource_version = self.resource_version.clone();
        let gateway_class_key = self.gateway_class_key.clone();
        let client_id = self.client_id.clone();
        let client_name = self.client_name.clone();

        tokio::spawn(async move {
            let mut from_version = *resource_version.read().unwrap();

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
                        tracing::error!(
                            kind = T::kind_name(),
                            client_id = %client_id,
                            error = %e,
                            "Failed to start watch"
                        );
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

                                                // Apply change using the existing method
                                                let version = resource.get_version();
                                                match change {
                                                    ResourceChange::InitAdd
                                                    | ResourceChange::EventAdd
                                                    | ResourceChange::EventUpdate => {
                                                        let mut cache_data = data.write().unwrap();
                                                        cache_data.insert(version.to_string(), resource);
                                                        let mut rv_guard = resource_version.write().unwrap();
                                                        if version > *rv_guard {
                                                            *rv_guard = version;
                                                        }
                                                    }
                                                    ResourceChange::EventDelete => {
                                                        let mut cache_data = data.write().unwrap();
                                                        cache_data.remove(&version.to_string());
                                                        let mut rv_guard = resource_version.write().unwrap();
                                                        if version > *rv_guard {
                                                            *rv_guard = version;
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
                            tracing::error!(
                                kind = T::kind_name(),
                                client_id = %client_id,
                                error = %e,
                                "Watch stream error, reconnecting"
                            );
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
