use crate::conf_sync::cache_client::cache_data::CacheData;
use crate::observe::metrics::{global_metrics, record_config_event_parse_error};
use edgion_common::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use edgion_common::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use edgion_resources::{
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

const EDGION_BACKEND_KIND: &str = "EdgionBackend";

fn is_edgion_backend<T: ResourceMeta>() -> bool {
    T::kind_name() == EDGION_BACKEND_KIND
}

fn canonical_gateway_api_type_meta<T: ResourceMeta>(value: &serde_json::Value) -> bool {
    let Some(source) =
        edgion_resources::gateway_api_support::canonical_sources().find(|source| source.kind == T::kind_name())
    else {
        return true;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    object.get("kind").and_then(serde_json::Value::as_str) == Some(source.kind)
        && object.get("apiVersion").and_then(serde_json::Value::as_str)
            == Some(format!("{}/{}", edgion_resources::gateway_api_support::GROUP, source.version).as_str())
}

fn clear_backend_cache<T: ResourceMeta>(cache_data: &Arc<RwLock<CacheData<T>>>, sync_version: u64) {
    let mut cache = cache_data.write();
    cache.reset(Vec::new(), sync_version);
    cache.set_not_ready();
}

fn valid_dns_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && value.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
}

fn valid_kube_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 253 && value.split('.').all(valid_dns_label)
}

fn raw_backend_key(value: &serde_json::Value) -> Option<String> {
    let metadata = value.as_object()?.get("metadata")?.as_object()?;
    let namespace = metadata.get("namespace")?.as_str()?;
    let name = metadata.get("name")?.as_str()?;
    (valid_dns_label(namespace) && valid_kube_name(name)).then(|| format!("{namespace}/{name}"))
}

fn take_watch_event_parts(
    mut event: serde_json::Value,
    response_sv: u64,
) -> Result<(ResourceChange, u64, serde_json::Value), &'static str> {
    let event_object = event.as_object_mut().ok_or("watch event must be an object")?;
    let event_type = event_object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or("watch event type must be a string")?;
    let change = match event_type {
        "add" => ResourceChange::EventAdd,
        "update" => ResourceChange::EventUpdate,
        "delete" => ResourceChange::EventDelete,
        _ => return Err("unknown watch event type"),
    };
    let event_sv = event_object
        .get("sync_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(response_sv);
    let event_data = event_object.remove("data").ok_or("watch event data is missing")?;
    Ok((change, event_sv, event_data))
}

#[derive(Default)]
struct WatchLogIdentity {
    ns: Option<String>,
    name: Option<String>,
    resource_version: Option<String>,
}

impl WatchLogIdentity {
    fn from_event(event: &serde_json::Value) -> Self {
        let metadata = &event["data"]["metadata"];
        // Malformed input is not trusted metadata. Copy only bounded identifiers,
        // never the payload or the Display text of a deserialization error.
        Self {
            ns: metadata["namespace"]
                .as_str()
                .filter(|value| valid_dns_label(value))
                .map(str::to_owned),
            name: metadata["name"]
                .as_str()
                .filter(|value| valid_kube_name(value))
                .map(str::to_owned),
            resource_version: metadata["resourceVersion"]
                .as_str()
                .filter(|value| {
                    !value.is_empty()
                        && value.len() <= 128
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
                })
                .map(str::to_owned),
        }
    }
}

fn record_watch_parse_failure<T: ResourceMeta>(identity: &WatchLogIdentity, sync_version: u64, error: &'static str) {
    record_config_event_parse_error(T::kind_name());
    tracing::error!(
        kind = T::kind_name(),
        ns = identity.ns.as_deref(),
        name = identity.name.as_deref(),
        resourceVersion = identity.resource_version.as_deref(),
        syncVersion = sync_version,
        error,
        "Failed to decode watch configuration; relisting resources"
    );
}

fn apply_generic_watch_payload<T>(
    cache_data: &Arc<RwLock<CacheData<T>>>,
    data: &str,
    response_sv: u64,
) -> Result<(), ()>
where
    T: ResourceMeta + Resource + Clone + Send + 'static + serde::de::DeserializeOwned,
{
    let events: Vec<serde_json::Value> = match serde_json::from_str(data) {
        Ok(events) => events,
        Err(_) => {
            record_watch_parse_failure::<T>(
                &WatchLogIdentity::default(),
                response_sv,
                "watch response must be a valid JSON event array",
            );
            return Err(());
        }
    };

    // Keep successful events, but stop at the first failure so the caller relists
    // before consuming the tail or any newer response from this stream.
    let mut cache = cache_data.write();
    for event in events {
        let identity = WatchLogIdentity::from_event(&event);
        let event_sv = event["sync_version"].as_u64().unwrap_or(response_sv);
        let (change, event_sv, event_data) = match take_watch_event_parts(event, event_sv) {
            Ok(parts) => parts,
            Err(error) => {
                record_watch_parse_failure::<T>(&identity, event_sv, error);
                return Err(());
            }
        };

        if !canonical_gateway_api_type_meta::<T>(&event_data) {
            record_watch_parse_failure::<T>(
                &identity,
                event_sv,
                "watch resource has noncanonical Gateway API TypeMeta",
            );
            return Err(());
        }
        match serde_json::from_value::<T>(event_data) {
            Ok(mut resource) => {
                resource.pre_parse();
                edgion_resources::resource::meta::set_sync_version(resource.meta_mut(), event_sv);
                tracing::debug!(kind = T::kind_name(), name = ?resource.name_any(), namespace = ?resource.namespace(), rv = resource.get_version(), sv = event_sv, "Received resource from watch event");
                let resource_key = resource.key_name();
                match change {
                    ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                        cache.insert(resource_key.clone(), resource);
                        cache.add_compress_event(resource_key, change);
                    }
                    ResourceChange::EventDelete => {
                        cache.remove(&resource_key);
                        cache.add_compress_event(resource_key, change);
                    }
                    ResourceChange::InitAdd | ResourceChange::InitStart | ResourceChange::InitDone => {}
                }
            }
            Err(_) => {
                record_watch_parse_failure::<T>(
                    &identity,
                    event_sv,
                    "watch resource does not match the resource schema",
                );
                return Err(());
            }
        }
    }
    Ok(())
}

fn decode_list_payload<T>(data: &[u8]) -> Result<Vec<T>, &'static str>
where
    T: ResourceMeta + serde::de::DeserializeOwned,
{
    if edgion_resources::gateway_api_support::canonical_sources().any(|source| source.kind == T::kind_name()) {
        let raw_resources: Vec<serde_json::Value> =
            serde_json::from_slice(data).map_err(|_| "list payload must be a valid JSON resource array")?;
        if raw_resources
            .iter()
            .any(|resource| !canonical_gateway_api_type_meta::<T>(resource))
        {
            return Err("list contains noncanonical Gateway API TypeMeta");
        }
        return raw_resources
            .into_iter()
            .map(|resource| serde_json::from_value(resource).map_err(|_| "list resource does not match the schema"))
            .collect();
    }
    if !is_edgion_backend::<T>() {
        return serde_json::from_slice(data).map_err(|_| "list payload does not match the resource array schema");
    }
    let raw_resources: Vec<serde_json::Value> =
        serde_json::from_slice(data).map_err(|_| "backend list must be a valid JSON resource array")?;
    if raw_resources.iter().any(|resource| raw_backend_key(resource).is_none()) {
        return Err("backend list contains an unsafe resource identity");
    }
    raw_resources
        .into_iter()
        .map(|resource| serde_json::from_value(resource).map_err(|_| "backend list resource does not match the schema"))
        .collect()
}

fn publish_list_payload<T>(
    cache_data: &Arc<RwLock<CacheData<T>>>,
    data: &[u8],
    sync_version: u64,
) -> Result<usize, &'static str>
where
    T: ResourceMeta + Resource + Clone + Send + 'static + serde::de::DeserializeOwned,
{
    let mut resources: Vec<T> = match decode_list_payload(data) {
        Ok(resources) => resources,
        Err(error) => {
            if is_edgion_backend::<T>() {
                clear_backend_cache(cache_data, sync_version);
            }
            return Err(error);
        }
    };

    for resource in resources.iter_mut() {
        resource.pre_parse();
        edgion_resources::resource::meta::set_sync_version(resource.meta_mut(), sync_version);
    }
    let count = resources.len();
    cache_data.write().reset(resources, sync_version);
    Ok(count)
}

fn mark_ready_after_list<T: ResourceMeta>(cache_data: &Arc<RwLock<CacheData<T>>>, is_ready: &mut bool) {
    if !*is_ready {
        cache_data.write().set_ready();
        *is_ready = true;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackendWatchBatchOutcome {
    Applied,
    Unsafe,
}

fn apply_backend_watch_batch<T>(
    cache_data: &Arc<RwLock<CacheData<T>>>,
    events: Vec<serde_json::Value>,
    response_sv: u64,
) -> BackendWatchBatchOutcome
where
    T: ResourceMeta + Resource + Clone + Send + 'static + serde::de::DeserializeOwned,
{
    let mut cache = cache_data.write();
    for event in events {
        let Ok((change, event_sv, event_data)) = take_watch_event_parts(event, response_sv) else {
            cache.reset(Vec::new(), response_sv);
            cache.set_not_ready();
            return BackendWatchBatchOutcome::Unsafe;
        };
        let Some(resource_key) = raw_backend_key(&event_data) else {
            cache.reset(Vec::new(), response_sv);
            cache.set_not_ready();
            return BackendWatchBatchOutcome::Unsafe;
        };

        match serde_json::from_value::<T>(event_data) {
            Ok(mut resource) => {
                resource.pre_parse();
                edgion_resources::resource::meta::set_sync_version(resource.meta_mut(), event_sv);
                match change {
                    ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                        cache.insert(resource_key.clone(), resource);
                        cache.add_compress_event(resource_key, change);
                    }
                    ResourceChange::EventDelete => {
                        cache.remove(&resource_key);
                        cache.add_compress_event(resource_key, change);
                    }
                    ResourceChange::InitAdd | ResourceChange::InitStart | ResourceChange::InitDone => {}
                }
            }
            Err(_) => {
                record_config_event_parse_error(T::kind_name());
                cache.remove(&resource_key);
                cache.add_compress_event(resource_key.clone(), ResourceChange::EventDelete);
                tracing::error!(
                    kind = T::kind_name(),
                    key_name = %resource_key,
                    error_class = "invalid_watch_resource",
                    "Withdrew malformed backend watch resource"
                );
            }
        }
    }
    BackendWatchBatchOutcome::Applied
}

fn apply_backend_watch_payload<T>(
    cache_data: &Arc<RwLock<CacheData<T>>>,
    data: &str,
    response_sv: u64,
) -> BackendWatchBatchOutcome
where
    T: ResourceMeta + Resource + Clone + Send + 'static + serde::de::DeserializeOwned,
{
    let Ok(events) = serde_json::from_str::<Vec<serde_json::Value>>(data) else {
        record_config_event_parse_error(T::kind_name());
        clear_backend_cache(cache_data, response_sv);
        return BackendWatchBatchOutcome::Unsafe;
    };
    apply_backend_watch_batch(cache_data, events, response_sv)
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
        let list_request = tonic::Request::new(edgion_common::conf_sync::proto::ListRequest {
            key: gateway_class_key.to_string(),
            kind: T::kind_name().to_string(),
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

        let count = match publish_list_payload(cache_data, &list_data.data, list_data.sync_version) {
            Ok(count) => count,
            Err(error) => {
                if is_edgion_backend::<T>() {
                    tracing::error!(
                        kind = T::kind_name(),
                        error_class = "invalid_list_payload",
                        context = log_context,
                        "Rejected backend list and cleared runtime state"
                    );
                    return Err(tonic::Status::internal("invalid EdgionBackend list payload"));
                }
                tracing::error!(kind = T::kind_name(), error = %error, context = log_context, "Failed to parse list response");
                return Err(tonic::Status::internal(format!(
                    "Failed to parse list response: {error}"
                )));
            }
        };
        tracing::info!(kind = T::kind_name(), count, "Parsed and published resources");

        Ok(ListResult {
            sync_version: list_data.sync_version,
            server_id: list_data.server_id,
        })
    }

    /// Start watching resources from the gRPC conf server.
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

            let mut is_first_iteration = true;
            // Set when the reload-generation token cancelled the previous cycle
            // (cert reload or leader rotation). The channel swap is deliberate
            // and already complete, so the next cycle must relist IMMEDIATELY at
            // the initial backoff — sleeping at the current backoff (up to 32s)
            // here is what would stretch a failover's per-kind resync tail past
            // the acceptance budget (spec §7.2 item 5).
            let mut generation_reloaded = false;
            loop {
                // === Backoff before list ===
                // Skip on first iteration to avoid delaying startup.
                // Add jitter (0-3s) on retries to spread requests and prevent relist storms.
                if !is_first_iteration {
                    match resync_wait_secs(generation_reloaded, backoff_secs) {
                        None => {
                            generation_reloaded = false;
                            backoff_secs = BACKOFF_INIT_SECS;
                        }
                        Some(wait_secs) => {
                            let jitter_ms = rand::rng().random_range(0..3000);
                            let wait_duration =
                                std::time::Duration::from_secs(wait_secs) + std::time::Duration::from_millis(jitter_ms);
                            tracing::info!(
                                kind = T::kind_name(),
                                backoff_secs = wait_secs,
                                jitter_ms = jitter_ms,
                                "Waiting before list"
                            );
                            // Selected on the CURRENT generation token (review
                            // finding): a rotation/reload landing mid-backoff must
                            // not wait out the remaining sleep (up to 32s) before
                            // relisting on the already-swapped channel. A token
                            // that is already cancelled resolves immediately.
                            let sleep_token = crate::conf_sync::conf_client::current_reload_generation();
                            tokio::select! {
                                _ = tokio::time::sleep(wait_duration) => {}
                                _ = sleep_token.cancelled() => {
                                    tracing::info!(
                                        kind = T::kind_name(),
                                        "channel reinstalled during backoff; relisting immediately"
                                    );
                                    backoff_secs = BACKOFF_INIT_SECS;
                                }
                            }
                        }
                    }
                }
                is_first_iteration = false;

                // Capture the reload-generation token BEFORE cloning the client so it
                // governs both the list and the watch of this cycle. Invariant: token
                // captured before client-clone ⟹ if the token is the new generation,
                // the clone below happens after the reload's set_all_grpc_clients swap
                // and reads the new client; if the token is the old generation, it is
                // already cancelled (or will be shortly) and the select in the watch
                // loop below breaks out to re-clone the new client. Capturing the token
                // after the clone would let the reload complete during the RPC
                // round-trip and hand us the new (uncancelled) token for the old
                // client, leaking a stream on the retired channel. Mirrors the token-
                // before-handle order in `run_leader_session` (grpc_client.rs).
                let gen_token = crate::conf_sync::conf_client::current_reload_generation();

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
                // Selected on the generation token so an in-flight list against a
                // dead/rotated-away peer does not burn its full timeout before the
                // next cycle picks up the new channel (spec §7.2 item 5).
                let list_outcome = tokio::select! {
                    _ = gen_token.cancelled() => {
                        tracing::info!(kind = T::kind_name(), "channel reinstalled during list; relisting on the new channel");
                        generation_reloaded = true;
                        continue;
                    }
                    r = Self::list_and_reset(&mut client, "", &cache_data, "watch relist", "") => r,
                };
                let (from_version, current_server_id) = match list_outcome {
                    Ok(list_result) => {
                        // Reset backoff on success
                        backoff_secs = BACKOFF_INIT_SECS;

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

                        if let Some(client) = crate::conf_sync::get_global_config_client() {
                            client.set_current_server_id(list_result.server_id.clone());
                        }

                        // Set ready after first successful list
                        if !is_ready {
                            mark_ready_after_list(&cache_data, &mut is_ready);
                            tracing::info!(kind = T::kind_name(), "Cache is ready");
                        }

                        (list_result.sync_version, list_result.server_id)
                    }
                    Err(e) => {
                        if is_edgion_backend::<T>() {
                            is_ready = false;
                        }
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

                    let request = tonic::Request::new(edgion_common::conf_sync::proto::WatchRequest {
                        key: String::new(),
                        kind: T::kind_name().to_string(),
                        client_id: client_id.to_string(),
                        client_name: client_name.to_string(),
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

                    // gen_token was captured at the top of the outer cycle (before the
                    // list-clone) so it governs both the list and this watch. When the
                    // client TLS channel is hot-reloaded the token is cancelled,
                    // breaking us out of the message loop so the outer loop re-clones
                    // the (already-swapped) new channel and re-lists.
                    loop {
                        let watch_msg = tokio::select! {
                            _ = gen_token.cancelled() => {
                                tracing::info!(kind = T::kind_name(), "channel reinstalled (TLS reload or rotation), breaking watch to reconnect");
                                generation_reloaded = true;
                                break 'watch_block;
                            }
                            msg = stream.message() => msg,
                        };
                        match watch_msg {
                            Ok(Some(watch_response)) => {
                                // Check for server-side protocol errors first
                                if !watch_response.err.is_empty() {
                                    tracing::warn!(
                                        kind = T::kind_name(),
                                        error = watch_response.err,
                                        "Received error signal from server, re-listing"
                                    );
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

                                if is_edgion_backend::<T>() {
                                    if apply_backend_watch_payload(&cache_data, &watch_response.data, response_sv)
                                        == BackendWatchBatchOutcome::Unsafe
                                    {
                                        is_ready = false;
                                        tracing::error!(
                                            kind = T::kind_name(),
                                            error_class = "invalid_watch_batch",
                                            "Rejected backend watch batch and cleared runtime state"
                                        );
                                        break 'watch_block;
                                    }
                                    continue;
                                }

                                if apply_generic_watch_payload(&cache_data, &watch_response.data, response_sv).is_err()
                                {
                                    break 'watch_block;
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

/// Pre-list wait decision (spec §7.2 item 5): `None` = the previous cycle ended
/// because the reload-generation token was cancelled (cert reload / leader
/// rotation) — the channel swap is deliberate and complete, so relist
/// immediately and reset the backoff; `Some(secs)` = a real failure, sleep the
/// current backoff (plus jitter at the call site).
fn resync_wait_secs(generation_reloaded: bool, backoff_secs: u64) -> Option<u64> {
    if generation_reloaded {
        None
    } else {
        Some(backoff_secs)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_sync/cache_client/event_dispatch_test.rs"]
mod tests;
