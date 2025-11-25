use crate::core::conf_sync::cache_server::EventDispatch;
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::{ResourceMeta, WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED};
use kube::{Resource, ResourceExt};
use std::sync::{Arc, RwLock};
use tonic::transport::Channel;

use super::cache::{CacheData, ClientCache};

impl<T: ResourceMeta + Resource + Clone + Send + 'static> EventDispatch<T> for ClientCache<T> {
    fn apply_change(&self, change: ResourceChange, resource: T)
    where
        T: Resource + Send + 'static,
    {
        // Extract resource key and add event to compress_events
        let key = resource.key_name();
        {
            let mut compress_events = self.compress_events.write().unwrap();
            compress_events.add_event(key, change);
        }

        let version = resource.get_version();
        tracing::info!(component = "cache_client", event = "apply_change", change = ?change, kind = std::any::type_name::<T>(), name = ?resource.name_any(), namespace = ?resource.namespace(), version = resource.get_version(), "Applying change to cache");

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
        let mut cache = self.cache_data.write().unwrap();
        cache.ready = true;
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

        tracing::info!(kind = T::kind_name(), count = resources.len(), "Parsed resources");

        // Use reset method to rebuild cache with fresh data
        {
            let mut cache = cache_data.write().unwrap();
            cache.reset(resources, list_data.resource_version);
        }

        Ok(list_data.resource_version)
    }

    // /// Sync resources from gRPC server
    // pub async fn sync(&self) -> Result<(), tonic::Status> {
    //     let mut client_guard = self.grpc_client.write().await;
    //     let client = client_guard
    //         .as_mut()
    //         .ok_or_else(|| tonic::Status::internal("gRPC client not initialized"))?;
    //
    //     Self::list_and_reset(
    //         client,
    //         self.gateway_class_key.as_ref(),
    //         &self.cache_data,
    //         "sync",
    //     )
    //     .await?;
    //
    //     tracing::info!(kind = T::kind_name(), "Finished syncing");
    //     Ok(())
    // }

    /// Start watching resources from gRPC server
    pub async fn start_watch(&self) -> Result<(), tonic::Status> {
        let grpc_client = self.grpc_client.clone();
        let cache_data = self.cache_data.clone();
        let gck = self.gateway_class_key.clone();
        let client_id = self.client_id.clone();
        let client_name = self.client_name.clone();

        tokio::spawn(async move {
            let mut is_ready = false;
            // Outer loop: perform list operation
            loop {
                // Get gRPC client
                let mut client_guard = grpc_client.write().await;
                let client = match client_guard.as_mut() {
                    Some(c) => c,
                    None => {
                        tracing::error!(kind = T::kind_name(), "gRPC client not initialized");
                        drop(client_guard);
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        continue;
                    }
                };

                // Perform list_and_reset to get latest resource version
                let from_version = match Self::list_and_reset(client, gck.as_ref(), &cache_data, "watch relist").await {
                    Ok(resource_version) => {
                        let count = {
                            let cache = cache_data.read().unwrap();
                            cache.data.len()
                        };
                        tracing::info!(kind = T::kind_name(), count = count, version = resource_version, "List completed, starting watch");
                        
                        // Set ready after first successful list
                        if !is_ready {
                            let mut cache = cache_data.write().unwrap();
                            cache.ready = true;
                            is_ready = true;
                            tracing::info!(kind = T::kind_name(), "Cache is ready");
                        }
                        
                        resource_version
                    }
                    Err(e) => {
                        tracing::error!(kind = T::kind_name(), error = %e, "Failed to perform list, retrying");
                        drop(client_guard);
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        continue;
                    }
                };

                drop(client_guard);

                // Inner loop: perform watch operation
                'watch_loop: loop {
                    let mut client_guard = grpc_client.write().await;
                    let client = match client_guard.as_mut() {
                        Some(c) => c,
                        None => {
                            tracing::error!(kind = T::kind_name(), "gRPC client not initialized for watch");
                            drop(client_guard);
                            break 'watch_loop; // Return to outer loop
                        }
                    };

                    let request = tonic::Request::new(crate::core::conf_sync::proto::WatchRequest {
                        key: gck.as_ref().clone(),
                        kind: T::resource_kind() as i32,
                        client_id: client_id.as_ref().clone(),
                        client_name: client_name.as_ref().clone(),
                        from_version,
                    });

                    let mut stream = match client.watch(request).await {
                        Ok(response) => {
                            tracing::info!(kind = T::kind_name(), from_version = from_version, "Watch started");
                            response.into_inner()
                        }
                        Err(e) => {
                            tracing::error!(kind = T::kind_name(), error = %e, from_version = from_version, "Failed to start watch");
                            drop(client_guard);
                            break 'watch_loop; // Return to outer loop to re-list
                        }
                    };

                    drop(client_guard);

                    // Process watch stream messages
                    loop {
                        match stream.message().await {
                            Ok(Some(watch_response)) => {
                                let _ = watch_response.resource_version; // Track version from response

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
                                tracing::info!(kind = T::kind_name(), "Watch stream ended");
                                break 'watch_loop; // Return to outer loop to re-list
                            }
                            Err(e) => {
                                let error_message = e.message();
                                if error_message.contains(WATCH_ERR_VERSION_UNEXPECTED)
                                    || error_message.contains(WATCH_ERR_TOO_OLD_VERSION)
                                {
                                    tracing::warn!(kind = T::kind_name(), error = %e, "Watch stream version error");
                                } else {
                                    tracing::error!(kind = T::kind_name(), error = %e, "Watch stream error");
                                }
                                break 'watch_loop; // Return to outer loop to re-list
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }
}

