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
mod tests {
    //! Unit tests for the `CacheEventDispatch` impl on `ClientCache`.
    //!
    //! Covers cache mutations, generic Watch diagnostics and wire decoding, and
    //! Backend withdrawal/publication with an in-process handler. The full
    //! watch/relist/backoff loop in `start_watch` needs integration coverage.
    use super::*;
    use crate::ai::AiBackendStore;
    use edgion_common::conf_sync::proto::{ListRequest, ListResponse, WatchResponse};
    use edgion_common::conf_sync::traits::{CacheEventDispatch, ConfHandler, ResourceChange};
    use edgion_resources::resources::{BackendTLSPolicy, EdgionBackend, Gateway, GatewayClass};
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::core::ObjectMeta;
    use std::collections::{HashMap, HashSet};

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<parking_lot::Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_watch_diagnostics(run: impl FnOnce()) -> (String, String) {
        let buffer = LogBuffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        metrics::with_local_recorder(&recorder, || tracing::subscriber::with_default(subscriber, run));
        let logs = String::from_utf8(buffer.0.lock().clone()).unwrap();
        (logs, recorder.handle().render())
    }

    #[test]
    fn generic_watch_stops_at_first_failure_and_keeps_valid_prefix() {
        let cache = new_client_cache();
        cache.set_ready();
        let mut malformed = serde_json::to_value(make_cm("ns1", "bad")).unwrap();
        malformed["metadata"]["resourceVersion"] = serde_json::json!("123");
        // Serde's Display text includes this string when decoding bool fails.
        malformed["immutable"] = serde_json::json!("private-password-sentinel");
        let batch = serde_json::json!([
            {"type": "add", "data": make_cm("ns1", "before")},
            {"type": "update", "sync_version": 41, "data": malformed},
            {"type": "private-envelope-sentinel", "data": {}},
            {"type": "add", "data": make_cm("ns1", "after")}
        ]);
        let (logs, metrics) = capture_watch_diagnostics(|| {
            assert!(apply_generic_watch_payload(&cache.cache_data, &batch.to_string(), 42).is_err());
        });
        assert_eq!(cached_keys(&cache), ["ns1/before"]);
        assert!(cache.is_ready());
        assert_eq!(logs.lines().count(), 1, "{logs}");
        for field in [
            "kind=\"ConfigMap\"",
            "ns=\"ns1\"",
            "name=\"bad\"",
            "resourceVersion=\"123\"",
            "syncVersion=41",
        ] {
            assert!(logs.contains(field), "{logs}");
        }
        assert!(logs.contains("watch resource does not match the resource schema"));
        assert!(!logs.contains("sentinel"));
        assert!(
            metrics.contains("edgion_config_event_parse_errors_total{kind=\"ConfigMap\"} 1"),
            "{metrics}"
        );
    }

    #[test]
    fn generic_watch_envelope_and_batch_failures_are_observable_without_payloads() {
        let cache = new_client_cache();
        let cases = [
            (
                "not-json-private-sentinel",
                "watch response must be a valid JSON event array",
            ),
            ("[null]", "watch event must be an object"),
            (r#"[{"type":"private-sentinel","data":{}}]"#, "unknown watch event type"),
            (r#"[{"data":{}}]"#, "watch event type must be a string"),
            (r#"[{"type":"add"}]"#, "watch event data is missing"),
        ];
        for (payload, expected) in cases {
            let (logs, metrics) = capture_watch_diagnostics(|| {
                assert!(apply_generic_watch_payload(&cache.cache_data, payload, 42).is_err());
            });
            assert_eq!(logs.lines().count(), 1, "{logs}");
            assert!(logs.contains(expected) && logs.contains("syncVersion=42"), "{logs}");
            assert!(!logs.contains("sentinel"));
            assert!(
                metrics.contains("edgion_config_event_parse_errors_total{kind=\"ConfigMap\"} 1"),
                "{metrics}"
            );
        }
    }

    #[test]
    fn generic_watch_does_not_log_invalid_or_unbounded_metadata() {
        let cache = new_client_cache();
        let batch = serde_json::json!([{"type": "add", "data": {
            "metadata": {
                "namespace": "private-sentinel\nforged-log",
                "name": "private-sentinel".repeat(64),
                "resourceVersion": {"password": "private-sentinel"}
            },
            "immutable": "private-sentinel"
        }}]);
        let (logs, _) = capture_watch_diagnostics(|| {
            assert!(apply_generic_watch_payload(&cache.cache_data, &batch.to_string(), 42).is_err());
        });
        assert_eq!(logs.lines().count(), 1, "{logs}");
        assert!(!logs.contains("sentinel") && !logs.contains("forged-log"));
        assert!(logs.len() < 512);
    }

    #[test]
    fn generic_watch_round_trips_controller_events_and_resolved_tls_secret() {
        use edgion_common::conf_sync::types::{EventType, WatcherEvent};
        use edgion_resources::resources::EdgionTls;
        use k8s_openapi::{api::core::v1::Secret, ByteString};

        let mut tls: EdgionTls = serde_json::from_value(serde_json::json!({
            "metadata": {"namespace": "ns1", "name": "tls", "resourceVersion": "123"},
            "spec": {"hosts": ["example.com"], "secretRef": {"name": "cert"}}
        }))
        .unwrap();
        tls.spec.secret = Some(Secret {
            data: Some(std::collections::BTreeMap::from([(
                "tls.key".to_string(),
                ByteString(b"private-test-key".to_vec()),
            )])),
            ..Default::default()
        });
        let cache = Arc::new(RwLock::new(CacheData::<EdgionTls>::new()));
        // Match Controller watch_json: serialize the shared typed event array.
        for (event_type, sv, present) in [
            (EventType::Add, 41, true),
            (EventType::Update, 42, true),
            (EventType::Delete, 43, false),
        ] {
            let payload = serde_json::to_string(&vec![WatcherEvent {
                event_type,
                sync_version: sv,
                data: tls.clone(),
            }])
            .unwrap();
            let (logs, metrics) =
                capture_watch_diagnostics(|| apply_generic_watch_payload(&cache, &payload, 99).unwrap());
            assert!(logs.is_empty() && !metrics.contains("edgion_config_event_parse_errors_total"));
            let data = cache.read();
            assert_eq!(data.get("ns1/tls").is_some(), present);
            if let Some(stored) = data.get("ns1/tls") {
                assert_eq!(stored.metadata.resource_version.as_deref(), Some("123"));
                assert_eq!(
                    stored.spec.secret.as_ref().unwrap().data.as_ref().unwrap()["tls.key"].0,
                    b"private-test-key"
                );
                assert_eq!(stored.get_sync_version(), sv);
            }
        }
    }

    #[derive(Default)]
    struct RecordingMapHandler {
        data: parking_lot::Mutex<HashMap<String, Arc<ConfigMap>>>,
        publications: parking_lot::Mutex<Vec<&'static str>>,
    }

    impl ConfHandler<ConfigMap> for RecordingMapHandler {
        fn full_set(&self, data: &HashMap<String, Arc<ConfigMap>>) {
            *self.data.lock() = data.clone();
            self.publications.lock().push("full");
        }

        fn partial_update(
            &self,
            add: HashMap<String, Arc<ConfigMap>>,
            update: HashMap<String, Arc<ConfigMap>>,
            remove: HashSet<String>,
        ) {
            let mut data = self.data.lock();
            for key in remove {
                data.remove(&key);
            }
            data.extend(add);
            data.extend(update);
            self.publications.lock().push("partial");
        }
    }

    #[test]
    fn generic_relist_preserves_readiness_and_supersedes_pending_events() {
        // Exercise both possible callback orders around reset: an already
        // published valid prefix, and a prefix still waiting in the queue.
        for flush_before_reset in [false, true] {
            let cache = Arc::new(RwLock::new(CacheData::<ConfigMap>::new()));
            let handler = Arc::new(RecordingMapHandler::default());
            cache.write().set_conf_processor_for_test(Box::new(handler.clone()));
            let initial = serde_json::to_vec(&vec![make_cm("ns1", "old")]).unwrap();
            publish_list_payload(&cache, &initial, 10).unwrap();
            let mut ready = false;
            mark_ready_after_list(&cache, &mut ready);

            let batch = serde_json::json!([
                {"type": "delete", "data": make_cm("ns1", "old")},
                {"type": "unknown", "data": {}},
                {"type": "add", "data": make_cm("ns1", "tail")}
            ]);
            assert!(apply_generic_watch_payload(&cache, &batch.to_string(), 11).is_err());
            assert!(cache.read().is_empty());
            assert_eq!(cache.read().compressed_event_count_for_test(), 1);
            assert_eq!(cache.read().sync_version(), 10);
            assert!(cache.read().is_ready());
            assert!(handler.data.lock().contains_key("ns1/old"));

            // A failed replacement must not clear old runtime configuration or
            // publish partially decoded List data, and its error must be safe.
            let bad_list = serde_json::json!([
                make_cm("ns1", "must-not-publish"),
                {"metadata": {"name": "bad"}, "immutable": "private-list-password-sentinel"}
            ]);
            let error = publish_list_payload(&cache, &serde_json::to_vec(&bad_list).unwrap(), 12).unwrap_err();
            assert_eq!(error, "list payload does not match the resource array schema");
            assert_eq!(handler.data.lock().len(), 1);
            assert!(handler.data.lock().contains_key("ns1/old"));
            assert_eq!(cache.read().sync_version(), 10);
            assert!(cache.read().is_ready());
            if flush_before_reset {
                CacheData::flush_compressed_events_for_test(&cache);
                assert!(handler.data.lock().is_empty());
            }

            let replacement = serde_json::to_vec(&vec![make_cm("ns1", "tail")]).unwrap();
            publish_list_payload(&cache, &replacement, 20).unwrap();
            mark_ready_after_list(&cache, &mut ready);
            let publications = handler.publications.lock().len();
            assert_eq!(cache.read().compressed_event_count_for_test(), 0);
            CacheData::flush_compressed_events_for_test(&cache);
            assert_eq!(handler.publications.lock().len(), publications);
            assert_eq!(handler.data.lock().len(), 1);
            assert!(handler.data.lock().contains_key("ns1/tail"));
            assert!(cache.read().is_ready());
            assert_eq!(cache.read().sync_version(), 20);

            let resumed = serde_json::json!([{"type": "delete", "data": make_cm("ns1", "tail")}]);
            apply_generic_watch_payload(&cache, &resumed.to_string(), 21).unwrap();
            CacheData::flush_compressed_events_for_test(&cache);
            assert!(handler.data.lock().is_empty());
            assert_eq!(handler.publications.lock().last(), Some(&"partial"));
        }
    }

    struct RecordingBackendHandler {
        store: Arc<AiBackendStore>,
        full_set_sizes: parking_lot::Mutex<Vec<usize>>,
        partial_removes: parking_lot::Mutex<Vec<HashSet<String>>>,
    }

    impl RecordingBackendHandler {
        fn new(store: Arc<AiBackendStore>) -> Self {
            Self {
                store,
                full_set_sizes: parking_lot::Mutex::new(Vec::new()),
                partial_removes: parking_lot::Mutex::new(Vec::new()),
            }
        }
    }

    impl ConfHandler<EdgionBackend> for RecordingBackendHandler {
        fn full_set(&self, data: &HashMap<String, Arc<EdgionBackend>>) {
            self.full_set_sizes.lock().push(data.len());
            self.store.full_set(data);
        }

        fn partial_update(
            &self,
            add: HashMap<String, Arc<EdgionBackend>>,
            update: HashMap<String, Arc<EdgionBackend>>,
            remove: HashSet<String>,
        ) {
            self.partial_removes.lock().push(remove.clone());
            self.store.partial_update(add, update, remove);
        }
    }

    fn backend_cache_with_handler() -> (
        Arc<RwLock<CacheData<EdgionBackend>>>,
        Arc<AiBackendStore>,
        Arc<RecordingBackendHandler>,
    ) {
        let cache = Arc::new(RwLock::new(CacheData::new()));
        let store = Arc::new(AiBackendStore::new());
        let handler = Arc::new(RecordingBackendHandler::new(Arc::clone(&store)));
        cache
            .write()
            .set_conf_processor_for_test(Box::new(Arc::clone(&handler)));
        (cache, store, handler)
    }

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

    #[test]
    fn generation_cancel_relists_immediately_at_initial_backoff() {
        // Spec §7.2 item 5: a generation-cancel (cert reload / rotation) must
        // relist immediately (no sleep) — otherwise each kind sleeps at its
        // current backoff (up to 32s) after a failover, blowing the resync
        // budget. A real failure keeps sleeping at the current backoff.
        assert_eq!(super::resync_wait_secs(true, 32), None, "reload: skip the sleep");
        assert_eq!(super::resync_wait_secs(false, 8), Some(8), "failure: sleep the backoff");
    }

    #[test]
    fn backend_watch_rejects_invalid_batch_json() {
        assert!(serde_json::from_str::<Vec<serde_json::Value>>("not-json").is_err());
    }

    #[test]
    fn backend_watch_rejects_non_object_event() {
        assert!(take_watch_event_parts(serde_json::json!("add"), 1).is_err());
    }

    #[test]
    fn backend_watch_rejects_missing_type() {
        assert!(take_watch_event_parts(serde_json::json!({"data": {}}), 1).is_err());
    }

    #[test]
    fn backend_watch_rejects_non_string_type() {
        assert!(take_watch_event_parts(serde_json::json!({"type": 1, "data": {}}), 1).is_err());
    }

    #[test]
    fn backend_watch_rejects_unknown_type() {
        assert!(take_watch_event_parts(serde_json::json!({"type": "replace", "data": {}}), 1).is_err());
    }

    #[test]
    fn backend_watch_rejects_missing_data() {
        assert!(take_watch_event_parts(serde_json::json!({"type": "add"}), 1).is_err());
    }

    #[test]
    fn backend_watch_accepts_recognized_event_envelopes() {
        for (event_type, expected) in [
            ("add", ResourceChange::EventAdd),
            ("update", ResourceChange::EventUpdate),
            ("delete", ResourceChange::EventDelete),
        ] {
            let event = serde_json::json!({"type": event_type, "data": {}});
            assert_eq!(take_watch_event_parts(event, 1).unwrap().0, expected);
        }
    }

    #[test]
    fn watch_event_data_is_moved_without_deep_clone() {
        let payload = "x".repeat(4096);
        let event = serde_json::json!({
            "type": "update",
            "sync_version": 9,
            "data": {"payload": payload}
        });
        let original_pointer = event["data"]["payload"]
            .as_str()
            .expect("test payload must be a string")
            .as_ptr();

        let (change, sync_version, data) = take_watch_event_parts(event, 3).unwrap();
        assert_eq!(change, ResourceChange::EventUpdate);
        assert_eq!(sync_version, 9);
        assert_eq!(
            data["payload"]
                .as_str()
                .expect("moved payload must remain a string")
                .as_ptr(),
            original_pointer
        );
    }

    #[test]
    fn backend_watch_uses_only_valid_exact_key_for_malformed_resource_withdrawal() {
        assert_eq!(
            raw_backend_key(&serde_json::json!({"metadata": {"namespace": "ns-1", "name": "backend.example"}})),
            Some("ns-1/backend.example".to_string())
        );
        for unsafe_identity in [
            serde_json::json!({}),
            serde_json::json!({"metadata": {"name": "backend"}}),
            serde_json::json!({"metadata": {"namespace": "", "name": "backend"}}),
            serde_json::json!({"metadata": {"namespace": "ns", "name": "../backend"}}),
        ] {
            assert_eq!(raw_backend_key(&unsafe_identity), None);
        }
    }

    fn valid_backend_json() -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackend",
            "metadata": {"namespace": "ns-1", "name": "backend.example"},
            "spec": {
                "ai": {
                    "provider": "OpenAI",
                    "credentialPool": {
                        "credentials": [{"name": "primary", "secretRef": {"name": "key"}}]
                    },
                    "models": [{"name": "model-a"}]
                }
            }
        })
    }

    fn runtime_backend_json(name: &str) -> serde_json::Value {
        let mut backend = valid_backend_json();
        backend["metadata"]["name"] = serde_json::json!(name);
        backend["spec"]["ai"]["credentialPool"]["credentials"][0]["secret"] = serde_json::json!({
            "data": {"api-key": "c2VjcmV0"},
            "type": "Opaque"
        });
        backend
    }

    #[test]
    fn backend_list_validates_identity_before_typed_publication() {
        let valid = serde_json::to_vec(&vec![valid_backend_json()]).unwrap();
        let decoded = decode_list_payload::<EdgionBackend>(&valid).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].key_name(), "ns-1/backend.example");

        for metadata in [
            serde_json::json!({"name": "backend"}),
            serde_json::json!({"namespace": "ns", "name": ""}),
            serde_json::json!({"namespace": "Bad_Namespace", "name": "backend"}),
            serde_json::json!({"namespace": "ns", "name": "../backend"}),
        ] {
            let mut unsafe_backend = valid_backend_json();
            unsafe_backend["metadata"] = metadata;
            let payload = serde_json::to_vec(&vec![unsafe_backend]).unwrap();
            assert!(decode_list_payload::<EdgionBackend>(&payload).is_err());
        }
    }

    #[test]
    fn ordinary_resource_list_keeps_existing_identity_semantics() {
        let payload = serde_json::to_vec(&vec![serde_json::json!({"metadata": {"name": "cm"}})]).unwrap();
        assert_eq!(decode_list_payload::<ConfigMap>(&payload).unwrap().len(), 1);
    }

    #[test]
    fn gateway_api_list_rejects_noncanonical_type_meta_before_typed_parse() {
        let valid = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
            "metadata": {"name": "edge"},
            "spec": {"controllerName": "example.com/gateway"}
        });
        assert_eq!(
            decode_list_payload::<GatewayClass>(&serde_json::to_vec(&vec![valid.clone()]).unwrap())
                .unwrap()
                .len(),
            1
        );
        for (field, replacement) in [
            ("apiVersion", serde_json::json!("gateway.networking.k8s.io/v1beta1")),
            ("apiVersion", serde_json::json!(1)),
            ("kind", serde_json::json!("Gateway")),
        ] {
            let mut wrong = valid.clone();
            wrong[field] = replacement;
            assert!(decode_list_payload::<GatewayClass>(&serde_json::to_vec(&vec![wrong]).unwrap()).is_err());
        }
        for field in ["apiVersion", "kind"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(decode_list_payload::<GatewayClass>(&serde_json::to_vec(&vec![missing]).unwrap()).is_err());
        }
    }

    #[test]
    fn gateway_api_watch_rejects_beta_and_recovers_on_canonical_event() {
        let cache = Arc::new(RwLock::new(CacheData::<GatewayClass>::new()));
        let mut resource = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
            "metadata": {"name": "edge"},
            "spec": {"controllerName": "example.com/gateway"}
        });
        let event = |data| serde_json::to_string(&vec![serde_json::json!({"type": "add", "data": data})]).unwrap();
        resource["apiVersion"] = serde_json::json!("gateway.networking.k8s.io/v1beta1");
        assert!(apply_generic_watch_payload(&cache, &event(resource.clone()), 1).is_err());
        assert!(cache.read().is_empty());
        resource["apiVersion"] = serde_json::json!("gateway.networking.k8s.io/v1");
        assert!(apply_generic_watch_payload(&cache, &event(resource), 2).is_ok());
        assert_eq!(cache.read().len(), 1);
    }

    #[test]
    fn gateway_list_retains_sibling_when_address_value_is_absent_and_recovers() {
        let cache = Arc::new(RwLock::new(CacheData::<Gateway>::new()));
        let gateway = |name: &str, address: serde_json::Value| {
            serde_json::json!({
                "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
                "metadata": {"name": name, "namespace": "demo"},
                "spec": {"gatewayClassName": "edge", "addresses": [address],
                    "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]}
            })
        };
        let valid = gateway("valid", serde_json::json!({"type": "IPAddress", "value": "192.0.2.1"}));
        let absent = gateway("absent", serde_json::json!({"type": "IPAddress"}));
        let mixed = serde_json::to_vec(&vec![valid.clone(), absent]).unwrap();
        assert_eq!(publish_list_payload(&cache, &mixed, 1).unwrap(), 2);
        assert_eq!(cache.read().len(), 2);
        assert!(cache.read().get("demo/valid").is_some());
        assert_eq!(
            cache
                .read()
                .get("demo/absent")
                .unwrap()
                .spec
                .addresses
                .as_ref()
                .unwrap()[0]
                .value,
            None
        );
        assert!(cache.read().get("demo/absent").unwrap().spec.listeners.is_none());

        let repaired = gateway("absent", serde_json::json!({"type": "IPAddress", "value": "192.0.2.2"}));
        let list = serde_json::to_vec(&vec![valid, repaired]).unwrap();
        assert_eq!(publish_list_payload(&cache, &list, 2).unwrap(), 2);
        assert_eq!(
            cache
                .read()
                .get("demo/absent")
                .unwrap()
                .spec
                .addresses
                .as_ref()
                .unwrap()[0]
                .value
                .as_deref(),
            Some("192.0.2.2")
        );
        assert!(cache.read().get("demo/absent").unwrap().spec.listeners.is_some());
    }

    #[test]
    fn backend_tls_list_retains_sibling_with_unsupported_ca_name_and_recovers() {
        let cache = Arc::new(RwLock::new(CacheData::<BackendTLSPolicy>::new()));
        let policy = |name: &str, ca: &str| {
            serde_json::json!({
                "apiVersion": "gateway.networking.k8s.io/v1", "kind": "BackendTLSPolicy",
                "metadata": {"name": name, "namespace": "demo"},
                "spec": {"targetRefs": [{"group": "", "kind": "Service", "name": "backend"}],
                    "validation": {"hostname": "backend.example.com", "wellKnownCACertificates": ca}}
            })
        };
        let valid = policy("valid", "System");
        let unsupported = policy("unsupported", "example.com/Custom");
        let mixed = serde_json::to_vec(&vec![valid.clone(), unsupported]).unwrap();
        assert_eq!(publish_list_payload(&cache, &mixed, 1).unwrap(), 2);
        assert_eq!(cache.read().len(), 2);
        assert!(cache.read().get("demo/valid").is_some());
        assert!(matches!(
            cache
                .read()
                .get("demo/unsupported")
                .unwrap()
                .spec
                .validation
                .well_known_ca_certificates,
            Some(edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::Other(_))
        ));
        let repaired = policy("unsupported", "System");
        let list = serde_json::to_vec(&vec![valid, repaired]).unwrap();
        assert_eq!(publish_list_payload(&cache, &list, 2).unwrap(), 2);
        assert!(matches!(
            cache
                .read()
                .get("demo/unsupported")
                .unwrap()
                .spec
                .validation
                .well_known_ca_certificates,
            Some(edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::System)
        ));
    }

    #[test]
    fn kind_23_conf_sync_envelopes_require_nested_ai_spec() {
        let request = ListRequest {
            key: String::new(),
            kind: EdgionBackend::kind(&()).to_string(),
            expected_server_id: "server-a".to_string(),
        };
        assert_eq!(request.kind, "EdgionBackend");
        assert_eq!(edgion_resources::ResourceKind::EdgionBackend as i32, 23);

        let nested = valid_backend_json();
        let nested_response = ListResponse {
            data: serde_json::to_vec(&vec![nested.clone()]).unwrap().into(),
            sync_version: 7,
            server_id: "server-a".to_string(),
        };
        assert_eq!(
            decode_list_payload::<EdgionBackend>(&nested_response.data)
                .unwrap()
                .len(),
            1
        );
        let mut flat = nested.clone();
        flat["spec"] = flat["spec"]["ai"].take();
        let flat_response = ListResponse {
            data: serde_json::to_vec(&vec![flat.clone()]).unwrap().into(),
            sync_version: 8,
            server_id: "server-a".to_string(),
        };
        assert!(decode_list_payload::<EdgionBackend>(&flat_response.data).is_err());

        let nested_watch = WatchResponse {
            data: serde_json::to_string(&vec![serde_json::json!({"type": "update", "data": nested})]).unwrap(),
            sync_version: 9,
            err: String::new(),
            server_id: "server-a".to_string(),
        };
        let nested_events: Vec<serde_json::Value> = serde_json::from_str(&nested_watch.data).unwrap();
        let (_, _, nested_data) = take_watch_event_parts(nested_events.into_iter().next().unwrap(), 9).unwrap();
        assert!(serde_json::from_value::<EdgionBackend>(nested_data).is_ok());

        let flat_watch = WatchResponse {
            data: serde_json::to_string(&vec![serde_json::json!({"type": "update", "data": flat})]).unwrap(),
            sync_version: 10,
            err: String::new(),
            server_id: "server-a".to_string(),
        };
        let flat_events: Vec<serde_json::Value> = serde_json::from_str(&flat_watch.data).unwrap();
        let (_, _, flat_data) = take_watch_event_parts(flat_events.into_iter().next().unwrap(), 10).unwrap();
        assert!(raw_backend_key(&flat_data).is_some());
        assert!(serde_json::from_value::<EdgionBackend>(flat_data).is_err());
    }

    #[test]
    fn backend_fail_close_clears_cache_version_and_readiness() {
        let cache = Arc::new(RwLock::new(CacheData::<ConfigMap>::new()));
        {
            let mut data = cache.write();
            data.insert("ns/a".to_string(), make_cm("ns", "a"));
            data.set_ready();
        }
        clear_backend_cache(&cache, 42);
        let data = cache.read();
        assert!(data.is_empty());
        assert!(!data.is_ready());
        assert_eq!(data.sync_version(), 42);
    }

    #[test]
    fn backend_list_failure_withdraws_runtime_and_valid_retry_restores_readiness() {
        let (cache, store, handler) = backend_cache_with_handler();
        let valid_payload = serde_json::to_vec(&vec![runtime_backend_json("backend-a")]).unwrap();
        let mut loop_ready = false;

        assert_eq!(publish_list_payload(&cache, &valid_payload, 10).unwrap(), 1);
        mark_ready_after_list(&cache, &mut loop_ready);
        assert!(cache.read().is_ready());
        assert_eq!(store.keys(), vec!["ns-1/backend-a"]);

        cache
            .write()
            .add_compress_event("ns-1/backend-a".to_string(), ResourceChange::EventUpdate);
        assert_eq!(cache.read().compressed_event_count_for_test(), 1);

        assert!(publish_list_payload(&cache, b"not-json", 11).is_err());
        loop_ready = false;
        assert!(cache.read().is_empty());
        assert!(!cache.read().is_ready());
        assert_eq!(cache.read().sync_version(), 11);
        assert_eq!(cache.read().compressed_event_count_for_test(), 0);
        assert_eq!(store.count(), 0, "empty full_set must withdraw the runtime snapshot");
        assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0]);

        assert_eq!(publish_list_payload(&cache, &valid_payload, 12).unwrap(), 1);
        mark_ready_after_list(&cache, &mut loop_ready);
        assert!(loop_ready);
        assert!(cache.read().is_ready());
        assert_eq!(cache.read().sync_version(), 12);
        assert_eq!(store.keys(), vec!["ns-1/backend-a"]);
        assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0, 1]);
    }

    #[test]
    fn backend_watch_safe_typed_failure_withdraws_exact_key_only() {
        let (cache, store, handler) = backend_cache_with_handler();
        let initial = serde_json::to_vec(&vec![
            runtime_backend_json("backend-a"),
            runtime_backend_json("backend-b"),
        ])
        .unwrap();
        publish_list_payload(&cache, &initial, 20).unwrap();
        let mut loop_ready = false;
        mark_ready_after_list(&cache, &mut loop_ready);

        let mut malformed = runtime_backend_json("backend-a");
        malformed["spec"] = malformed["spec"]["ai"].take();
        let payload = serde_json::to_string(&vec![serde_json::json!({
            "type": "update",
            "sync_version": 21,
            "data": malformed
        })])
        .unwrap();

        assert_eq!(
            apply_backend_watch_payload(&cache, &payload, 21),
            BackendWatchBatchOutcome::Applied
        );
        assert!(
            cache.read().is_ready(),
            "a safe exact-key withdrawal does not poison the stream"
        );
        assert!(cache.read().get("ns-1/backend-a").is_none());
        assert!(cache.read().get("ns-1/backend-b").is_some());
        assert_eq!(cache.read().compressed_event_count_for_test(), 1);

        assert!(CacheData::flush_compressed_events_for_test(&cache));
        assert_eq!(store.keys(), vec!["ns-1/backend-b"]);
        assert_eq!(cache.read().compressed_event_count_for_test(), 0);
        assert_eq!(
            &*handler.partial_removes.lock(),
            &[HashSet::from(["ns-1/backend-a".to_string()])]
        );
    }

    #[test]
    fn backend_watch_unsafe_batch_aborts_later_events_and_relist_restores_runtime() {
        let (cache, store, handler) = backend_cache_with_handler();
        let initial = serde_json::to_vec(&vec![runtime_backend_json("backend-a")]).unwrap();
        publish_list_payload(&cache, &initial, 30).unwrap();
        let mut loop_ready = false;
        mark_ready_after_list(&cache, &mut loop_ready);

        cache
            .write()
            .add_compress_event("ns-1/backend-a".to_string(), ResourceChange::EventUpdate);
        let batch = serde_json::to_string(&vec![
            serde_json::json!({"type": "delete", "data": runtime_backend_json("backend-a")}),
            serde_json::json!({"type": "replace", "data": {}}),
            serde_json::json!({"type": "add", "data": runtime_backend_json("must-not-apply")}),
        ])
        .unwrap();

        assert_eq!(
            apply_backend_watch_payload(&cache, &batch, 31),
            BackendWatchBatchOutcome::Unsafe
        );
        loop_ready = false;
        assert!(cache.read().is_empty());
        assert!(!cache.read().is_ready());
        assert_eq!(cache.read().sync_version(), 31);
        assert_eq!(cache.read().compressed_event_count_for_test(), 0);
        assert_eq!(store.count(), 0);
        assert!(store.get_by_key("ns-1/must-not-apply").is_none());
        assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0]);

        let restored = serde_json::to_vec(&vec![runtime_backend_json("backend-restored")]).unwrap();
        publish_list_payload(&cache, &restored, 32).unwrap();
        mark_ready_after_list(&cache, &mut loop_ready);
        assert!(loop_ready);
        assert!(cache.read().is_ready());
        assert_eq!(store.keys(), vec!["ns-1/backend-restored"]);
        assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0, 1]);

        assert_eq!(
            apply_backend_watch_payload(&cache, "not-json", 33),
            BackendWatchBatchOutcome::Unsafe
        );
        loop_ready = false;
        assert!(cache.read().is_empty());
        assert!(!cache.read().is_ready());
        assert_eq!(store.count(), 0);
        assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0, 1, 0]);

        publish_list_payload(&cache, &restored, 34).unwrap();
        mark_ready_after_list(&cache, &mut loop_ready);
        assert!(loop_ready);
        assert!(cache.read().is_ready());
        assert_eq!(store.keys(), vec!["ns-1/backend-restored"]);
        assert_eq!(&*handler.full_set_sizes.lock(), &[1, 0, 1, 0, 1]);
    }
}
