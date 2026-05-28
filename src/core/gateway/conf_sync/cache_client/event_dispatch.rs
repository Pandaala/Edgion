use crate::core::common::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::common::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::core::gateway::conf_sync::cache_client::cache_data::CacheData;
use crate::core::gateway::observe::metrics::{global_metrics, record_config_event_parse_error};
use crate::types::{
    ResourceMeta, WATCH_ERR_NOT_READY, WATCH_ERR_SERVER_ID_MISMATCH, WATCH_ERR_SERVER_RELOAD,
    WATCH_ERR_TOO_OLD_VERSION, WATCH_ERR_VERSION_UNEXPECTED,
};
use kube::{Resource, ResourceExt};
use parking_lot::RwLock;
use rand::Rng;
use std::sync::Arc;
use tonic::transport::Channel;

use super::cache::ClientCache;

/// Result of list_and_reset operation
struct ListResult {
    sync_version: u64,
    server_id: String,
}

impl<T: ResourceMeta + Resource + Clone + Send + 'static> CacheEventDispatch<T> for ClientCache<T> {
    fn apply_change(&self, change: ResourceChange, resource: T, _sv: Option<u64>)
    where
        T: Resource + Send + 'static,
    {
        // Collect log fields before acquiring lock to avoid holding lock during formatting
        let name = resource.name_any();
        let namespace = resource.namespace();
        let version = resource.get_version();
        let kind = std::any::type_name::<T>();

        // Both compress_event and data update must be atomic under one lock so that
        // process_compressed_events cannot observe an event whose data is not yet present.
        let resource_key = resource.key_name();
        {
            let mut cache = self.cache_data.write();
            cache.add_compress_event(resource_key.clone(), change);
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

        tracing::debug!(component = "cache_client", event = "apply_change", change = ?change, kind = kind, name = ?name, namespace = ?namespace, version = version, "Applying change to cache");
    }

    fn set_ready(&self) {
        let mut cache = self.cache_data.write();
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
        let list_request = tonic::Request::new(crate::core::common::conf_sync::proto::ListRequest {
            key: gateway_class_key.to_string(),
            kind: T::kind_name().to_string(), // Use string kind name instead of enum
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

        for resource in resources.iter_mut() {
            resource.pre_parse();
            crate::types::resource::meta::set_sync_version(resource.meta_mut(), list_data.sync_version);
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
            let mut cache = cache_data.write();
            cache.reset(resources, list_data.sync_version);
        }

        Ok(ListResult {
            sync_version: list_data.sync_version,
            server_id: list_data.server_id,
        })
    }

    /// Start watching resources from gRPC conf_server_old
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
            let mut is_first_iteration = true;
            loop {
                // === Backoff before list ===
                // Skip on first iteration to avoid delaying startup.
                // Add jitter (0-3s) on retries to spread requests and prevent relist storms.
                if !is_first_iteration {
                    let jitter_ms = rand::rng().random_range(0..3000);
                    let wait_duration =
                        std::time::Duration::from_secs(backoff_secs) + std::time::Duration::from_millis(jitter_ms);
                    tracing::info!(
                        kind = T::kind_name(),
                        backoff_secs = backoff_secs,
                        jitter_ms = jitter_ms,
                        "Waiting before list"
                    );
                    tokio::time::sleep(wait_duration).await;
                }
                is_first_iteration = false;

                // Get gRPC conf_client — clone the handle out under a brief write-lock
                // acquisition so we never hold the async write guard across the RPC.
                // ConfigSyncClientService<Channel> wraps a tonic Channel which is cheaply
                // cloneable (shares the underlying connection pool).
                let mut client = {
                    let guard = grpc_client.write().await;
                    match guard.as_ref() {
                        Some(c) => c.clone(),
                        None => {
                            drop(guard);
                            tracing::error!(kind = T::kind_name(), "gRPC conf_client not initialized");
                            // Increase backoff on failure
                            backoff_secs = (backoff_secs * 2).min(BACKOFF_MAX_SECS);
                            continue;
                        }
                    }
                };

                // Perform list_and_reset to get latest sync version and server_id
                // First list doesn't need to validate server_id (pass empty string)
                let (from_version, current_server_id) =
                    match Self::list_and_reset(&mut client, "", &cache_data, "watch relist", "").await {
                        Ok(list_result) => {
                            // Reset backoff on success
                            backoff_secs = BACKOFF_INIT_SECS;

                            // Record relist metric
                            global_metrics().config_relist();

                            let count = {
                                let cache = cache_data.read();
                                cache.len()
                            };
                            tracing::info!(
                                kind = T::kind_name(),
                                count = count,
                                sync_version = list_result.sync_version,
                                server_id = %list_result.server_id,
                                "List completed, starting watch"
                            );

                            // Update global ConfigClient's server_id
                            if let Some(client) = crate::core::gateway::conf_sync::get_global_config_client() {
                                client.set_current_server_id(list_result.server_id.clone());
                            }

                            // Set ready after first successful list
                            if !is_ready {
                                let mut cache = cache_data.write();
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
                            // Increase backoff on failure
                            backoff_secs = (backoff_secs * 2).min(BACKOFF_MAX_SECS);
                            continue;
                        }
                    };

                // Watch block: perform watch operation, break to return to outer loop for re-list
                'watch_block: {
                    // Clone the client out under a brief write-lock acquisition; do not hold
                    // the async write guard across the watch RPC or the stream lifetime.
                    let mut client = {
                        let guard = grpc_client.write().await;
                        match guard.as_ref() {
                            Some(c) => c.clone(),
                            None => {
                                drop(guard);
                                tracing::error!(kind = T::kind_name(), "gRPC conf_client not initialized for watch");
                                break 'watch_block; // Return to outer loop
                            }
                        }
                    };

                    let request = tonic::Request::new(crate::core::common::conf_sync::proto::WatchRequest {
                        key: String::new(),
                        kind: T::kind_name().to_string(), // Use string kind name instead of enum
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
                            break 'watch_block; // Return to outer loop to re-list
                        }
                    };

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
                                    // Record metric if this is a reload signal
                                    if watch_response.err.contains(WATCH_ERR_SERVER_RELOAD) {
                                        global_metrics().config_reload_signal();
                                    }
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

                                let response_sv = watch_response.sync_version;

                                let events: Vec<serde_json::Value> = match serde_json::from_str(&watch_response.data) {
                                    Ok(events) => events,
                                    Err(e) => {
                                        record_config_event_parse_error(T::kind_name());
                                        tracing::error!(kind = T::kind_name(), error = %e, "Failed to parse watch response events");
                                        continue;
                                    }
                                };

                                // Hold a single write guard across all events in this WatchResponse
                                // to coalesce O(N) lock acquisitions into one batch.
                                let mut cache = cache_data.write();
                                for event in events {
                                    if let Some(event_type) = event.get("type").and_then(|v| v.as_str()) {
                                        if let Some(event_data) = event.get("data") {
                                            let event_sv = event
                                                .get("sync_version")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(response_sv);

                                            match serde_json::from_value::<T>(event_data.clone()) {
                                                Ok(mut resource) => {
                                                    resource.pre_parse();
                                                    crate::types::resource::meta::set_sync_version(
                                                        resource.meta_mut(),
                                                        event_sv,
                                                    );

                                                    tracing::debug!(kind = T::kind_name(), name = ?resource.name_any(), namespace = ?resource.namespace(), rv = resource.get_version(), sv = event_sv, "Received resource from watch event");
                                                    let change = match event_type {
                                                        "add" => ResourceChange::EventAdd,
                                                        "update" => ResourceChange::EventUpdate,
                                                        "delete" => ResourceChange::EventDelete,
                                                        _ => continue,
                                                    };

                                                    let resource_key = resource.key_name(); // Use namespace/name as key
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
                                                    record_config_event_parse_error(T::kind_name());
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

#[cfg(test)]
mod tests {
    //! Unit tests for the `CacheEventDispatch` impl on `ClientCache`.
    //!
    //! These tests exercise only the lock-protected mutation logic of
    //! `apply_change` and `set_ready` — the change kind dispatched against a
    //! freshly constructed `ClientCache` (without a configured processor or
    //! gRPC channel). The full watch/relist/backoff loop in `start_watch` is
    //! deferred to integration coverage — see
    //! `tasks/todo-test-cover/05-integration-gaps-controller.md` #3 (relist
    //! storm) and #4 (server_id failover).
    use super::*;
    use crate::core::common::conf_sync::traits::{CacheEventDispatch, ResourceChange};
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::core::ObjectMeta;

    fn make_cm(namespace: &str, name: &str) -> ConfigMap {
        ConfigMap {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn new_client_cache() -> ClientCache<ConfigMap> {
        ClientCache::<ConfigMap>::new("client-id".to_string(), "client-name".to_string())
    }

    fn cached_keys(cache: &ClientCache<ConfigMap>) -> Vec<String> {
        let mut keys = cache.keys();
        keys.sort();
        keys
    }

    #[test]
    fn init_add_inserts_resource_into_cache() {
        let cache = new_client_cache();
        cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
        assert_eq!(cached_keys(&cache), vec!["ns1/a".to_string()]);
    }

    #[test]
    fn event_add_inserts_resource_into_cache() {
        let cache = new_client_cache();
        cache.apply_change(ResourceChange::EventAdd, make_cm("ns1", "a"), None);
        let stored = cache.get("ns1/a").expect("present");
        assert_eq!(stored.metadata.name.as_deref(), Some("a"));
    }

    #[test]
    fn event_update_overwrites_existing_resource() {
        let cache = new_client_cache();
        let mut original = make_cm("ns1", "a");
        original.metadata.resource_version = Some("1".to_string());
        cache.apply_change(ResourceChange::EventAdd, original, None);

        let mut updated = make_cm("ns1", "a");
        updated.metadata.resource_version = Some("2".to_string());
        cache.apply_change(ResourceChange::EventUpdate, updated, None);

        assert_eq!(cache.len(), 1);
        let stored = cache.get("ns1/a").expect("present");
        assert_eq!(stored.metadata.resource_version.as_deref(), Some("2"));
    }

    #[test]
    fn event_delete_removes_resource_from_cache() {
        let cache = new_client_cache();
        cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
        assert_eq!(cache.len(), 1);
        cache.apply_change(ResourceChange::EventDelete, make_cm("ns1", "a"), None);
        assert!(cache.is_empty());
    }

    #[test]
    fn init_start_signal_does_not_mutate_data() {
        let cache = new_client_cache();
        cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
        // InitStart is a signal, not a data event — must not change stored data.
        cache.apply_change(ResourceChange::InitStart, make_cm("ns2", "b"), None);
        assert_eq!(cached_keys(&cache), vec!["ns1/a".to_string()]);
    }

    #[test]
    fn init_done_signal_does_not_mutate_data() {
        let cache = new_client_cache();
        cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
        // InitDone is a signal — same expectation as InitStart.
        cache.apply_change(ResourceChange::InitDone, make_cm("ns2", "b"), None);
        assert_eq!(cached_keys(&cache), vec!["ns1/a".to_string()]);
    }

    #[test]
    fn set_ready_marks_cache_ready() {
        let cache = new_client_cache();
        assert!(!cache.is_ready());
        CacheEventDispatch::set_ready(&cache);
        assert!(cache.is_ready());
    }

    #[test]
    fn multiple_resources_with_distinct_keys_coexist() {
        let cache = new_client_cache();
        cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "a"), None);
        cache.apply_change(ResourceChange::InitAdd, make_cm("ns1", "b"), None);
        cache.apply_change(ResourceChange::InitAdd, make_cm("ns2", "a"), None);
        // ns1/a and ns2/a share a name but different namespaces — both must
        // survive because key_name() includes the namespace prefix.
        assert_eq!(
            cached_keys(&cache),
            vec!["ns1/a".to_string(), "ns1/b".to_string(), "ns2/a".to_string()]
        );
    }
}
