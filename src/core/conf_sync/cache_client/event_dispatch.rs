use crate::core::conf_sync::cache_client::cache_data::CacheData;
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::{
    ResourceMeta, WATCH_ERR_NOT_READY, WATCH_ERR_SERVER_ID_MISMATCH, WATCH_ERR_TOO_OLD_VERSION,
    WATCH_ERR_VERSION_UNEXPECTED,
};
use kube::{Resource, ResourceExt};
use rand::Rng;
use std::sync::{Arc, RwLock};
use tonic::transport::Channel;

use super::cache::ClientCache;

/// Result of list_and_reset operation
struct ListResult {
    sync_version: u64,
    server_id: String,
}

impl<T: ResourceMeta + Resource + Clone + Send + 'static> CacheEventDispatch<T> for ClientCache<T> {
    fn apply_change(&self, change: ResourceChange, resource: T)
    where
        T: Resource + Send + 'static,
    {
        // Extract resource key and add event to compress_events
        let key = resource.key_name();
        {
            let mut cache = self.cache_data.write().unwrap();
            cache.add_compress_event(key, change);
        }

        tracing::info!(component = "cache_client", event = "apply_change", change = ?change, kind = std::any::type_name::<T>(), name = ?resource.name_any(), namespace = ?resource.namespace(), version = resource.get_version(), "Applying change to cache");

        let mut cache = self.cache_data.write().unwrap();
        let resource_key = resource.key_name(); // Use namespace/name as key
        match change {
            ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                cache.insert(resource_key, resource);
            }
            ResourceChange::EventDelete => {
                cache.remove(&resource_key);
            }
            ResourceChange::InitStart | ResourceChange::InitDone => {
                // Signal events: no data changes needed
            }
        }
    }

    fn set_ready(&self) {
        let mut cache = self.cache_data.write().unwrap();
        cache.set_ready();
    }
}

// Additional methods for ResourceMeta types
impl<T> ClientCache<T>
where
    T: ResourceMeta + Resource + Clone + Send + 'static,
{
    /// Internal helper function to perform list and reset cache
    /// Returns the resource version and server_id on success
    async fn list_and_reset(
        client: &mut ConfigSyncClientService<Channel>,
        gateway_class_key: &str,
        cache_data: &Arc<RwLock<CacheData<T>>>,
        log_context: &str,
        expected_server_id: &str,
    ) -> Result<ListResult, tonic::Status> {
        let list_request = tonic::Request::new(crate::core::conf_sync::proto::ListRequest {
            key: gateway_class_key.to_string(),
            kind: T::resource_kind() as i32,
            expected_server_id: expected_server_id.to_string(),
        });

        let response = client.list(list_request).await?;
        let list_data = response.into_inner();

        tracing::info!(
            kind = T::kind_name(),
            bytes = list_data.data.len(),
            sync_version = list_data.sync_version,
            server_id = %list_data.server_id,
            context = log_context,
            "Listing resources"
        );

        // Parse JSON array directly to concrete type
        let mut resources: Vec<T> = serde_json::from_str(&list_data.data).map_err(|e| {
            tracing::error!(kind = T::kind_name(), error = %e, context = log_context, "Failed to parse list response");
            tonic::Status::internal(format!("Failed to parse list response: {}", e))
        })?;

        // Pre-parse all resources to populate runtime-only fields
        for resource in resources.iter_mut() {
            resource.pre_parse();
        }

        tracing::info!(kind = T::kind_name(), count = resources.len(), "Parsed resources");

        // Debug: print all resource keys and sync_version
        let resource_keys: Vec<String> = resources.iter().map(|r| r.key_name()).collect();
        tracing::debug!(
            kind = T::kind_name(),
            sync_version = list_data.sync_version,
            resources = ?resource_keys,
            "Resetting cache with resources"
        );

        // Use reset method to rebuild cache with fresh data
        {
            let mut cache = cache_data.write().unwrap();
            cache.reset(resources, list_data.sync_version);
        }

        Ok(ListResult {
            sync_version: list_data.sync_version,
            server_id: list_data.server_id,
        })
    }

    /// Start watching resources from gRPC conf_server
    pub async fn start_watch(&self) -> Result<(), tonic::Status> {
        let grpc_client = self.grpc_client.clone();
        let cache_data = self.cache_data.clone();
        let client_id = self.client_id.clone();
        let client_name = self.client_name.clone();

        tokio::spawn(async move {
            let mut is_ready = false;

            // Exponential backoff: 2s -> 4s -> 8s -> 16s -> 32s (max)
            const BACKOFF_INIT_SECS: u64 = 2;
            const BACKOFF_MAX_SECS: u64 = 32;
            let mut backoff_secs = BACKOFF_INIT_SECS;

            // Outer loop: perform list operation
            loop {
                // === Backoff before list ===
                // Add jitter (0-3s) to spread requests and prevent relist storms
                let jitter_ms = rand::thread_rng().gen_range(0..3000);
                let wait_duration =
                    std::time::Duration::from_secs(backoff_secs) + std::time::Duration::from_millis(jitter_ms);
                tracing::info!(
                    kind = T::kind_name(),
                    backoff_secs = backoff_secs,
                    jitter_ms = jitter_ms,
                    "Waiting before list"
                );
                tokio::time::sleep(wait_duration).await;

                // Get gRPC conf_client
                let mut client_guard = grpc_client.write().await;
                let client = match client_guard.as_mut() {
                    Some(c) => c,
                    None => {
                        tracing::error!(kind = T::kind_name(), "gRPC conf_client not initialized");
                        drop(client_guard);
                        // Increase backoff on failure
                        backoff_secs = (backoff_secs * 2).min(BACKOFF_MAX_SECS);
                        continue;
                    }
                };

                // Perform list_and_reset to get latest sync version and server_id
                // First list doesn't need to validate server_id (pass empty string)
                let (from_version, current_server_id) =
                    match Self::list_and_reset(client, "", &cache_data, "watch relist", "").await {
                        Ok(list_result) => {
                            // Reset backoff on success
                            backoff_secs = BACKOFF_INIT_SECS;

                            let count = {
                                let cache = cache_data.read().unwrap();
                                cache.len()
                            };
                            tracing::info!(
                                kind = T::kind_name(),
                                count = count,
                                sync_version = list_result.sync_version,
                                server_id = %list_result.server_id,
                                "List completed, starting watch"
                            );

                            // Set ready after first successful list
                            if !is_ready {
                                let mut cache = cache_data.write().unwrap();
                                cache.set_ready();
                                is_ready = true;
                                tracing::info!(kind = T::kind_name(), "Cache is ready");
                            }

                            (list_result.sync_version, list_result.server_id)
                        }
                        Err(e) => {
                            let error_message = e.message();
                            if error_message.contains(WATCH_ERR_NOT_READY) {
                                tracing::warn!(kind = T::kind_name(), error = %e, "Server not ready, will retry");
                            } else if error_message.contains(WATCH_ERR_SERVER_ID_MISMATCH) {
                                tracing::warn!(kind = T::kind_name(), error = %e, "Server ID mismatch, will relist");
                            } else {
                                tracing::error!(kind = T::kind_name(), error = %e, "Failed to perform list, retrying");
                            }
                            drop(client_guard);
                            // Increase backoff on failure
                            backoff_secs = (backoff_secs * 2).min(BACKOFF_MAX_SECS);
                            continue;
                        }
                    };

                drop(client_guard);

                // Watch block: perform watch operation, break to return to outer loop for re-list
                'watch_block: {
                    let mut client_guard = grpc_client.write().await;
                    let client = match client_guard.as_mut() {
                        Some(c) => c,
                        None => {
                            tracing::error!(kind = T::kind_name(), "gRPC conf_client not initialized for watch");
                            drop(client_guard);
                            break 'watch_block; // Return to outer loop
                        }
                    };

                    let request = tonic::Request::new(crate::core::conf_sync::proto::WatchRequest {
                        key: String::new(),
                        kind: T::resource_kind() as i32,
                        client_id: client_id.as_ref().clone(),
                        client_name: client_name.as_ref().clone(),
                        from_version,
                        expected_server_id: current_server_id.clone(),
                    });

                    let mut stream = match client.watch(request).await {
                        Ok(response) => {
                            tracing::info!(kind = T::kind_name(), from_version = from_version, "Watch started");
                            response.into_inner()
                        }
                        Err(e) => {
                            tracing::error!(kind = T::kind_name(), error = %e, from_version = from_version, "Failed to start watch");
                            drop(client_guard);
                            break 'watch_block; // Return to outer loop to re-list
                        }
                    };

                    drop(client_guard);

                    // Process watch stream messages
                    loop {
                        match stream.message().await {
                            Ok(Some(watch_response)) => {
                                // Check for server-side protocol errors first
                                if !watch_response.err.is_empty() {
                                    tracing::warn!(
                                        kind = T::kind_name(),
                                        error = watch_response.err,
                                        "Received error signal from server, re-listing"
                                    );
                                    break 'watch_block;
                                }

                                // Check for server_id change (server restart/failover detection)
                                // Always trigger relist when server instance changes
                                if !watch_response.server_id.is_empty() && watch_response.server_id != current_server_id
                                {
                                    tracing::warn!(
                                        kind = T::kind_name(),
                                        last_server_id = %current_server_id,
                                        new_server_id = %watch_response.server_id,
                                        "Server instance changed, triggering relist"
                                    );
                                    break 'watch_block; // Return to outer loop to re-list
                                }

                                let _ = watch_response.sync_version; // Track version from response

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
                                                Ok(mut resource) => {
                                                    // Pre-parse to populate runtime-only fields
                                                    resource.pre_parse();

                                                    tracing::info!(kind = T::kind_name(), name = ?resource.name_any(), namespace = ?resource.namespace(), version = resource.get_version(), "Received resource from watch event");
                                                    let change = match event_type {
                                                        "add" => ResourceChange::EventAdd,
                                                        "update" => ResourceChange::EventUpdate,
                                                        "delete" => ResourceChange::EventDelete,
                                                        _ => continue,
                                                    };

                                                    // Apply change using single lock
                                                    let resource_key = resource.key_name(); // Use namespace/name as key
                                                    let mut cache = cache_data.write().unwrap();
                                                    match change {
                                                        ResourceChange::InitAdd
                                                        | ResourceChange::EventAdd
                                                        | ResourceChange::EventUpdate => {
                                                            cache.insert(resource_key.clone(), resource);
                                                            // Add compress event to trigger partial_update
                                                            cache.add_compress_event(resource_key, change);
                                                        }
                                                        ResourceChange::EventDelete => {
                                                            cache.remove(&resource_key);
                                                            // Add compress event to trigger partial_update
                                                            cache.add_compress_event(resource_key, change);
                                                        }
                                                        ResourceChange::InitStart | ResourceChange::InitDone => {
                                                            // Signal events: not expected in watch stream
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
                                break 'watch_block; // Return to outer loop to re-list
                            }
                            Err(e) => {
                                let error_message = e.message();
                                if error_message.contains(WATCH_ERR_VERSION_UNEXPECTED)
                                    || error_message.contains(WATCH_ERR_TOO_OLD_VERSION)
                                    || error_message.contains(WATCH_ERR_NOT_READY)
                                    || error_message.contains(WATCH_ERR_SERVER_ID_MISMATCH)
                                {
                                    tracing::warn!(kind = T::kind_name(), error = %e, "Watch stream recoverable error, will retry");
                                } else {
                                    tracing::error!(kind = T::kind_name(), error = %e, "Watch stream error");
                                }
                                break 'watch_block; // Return to outer loop to re-list
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }
}
