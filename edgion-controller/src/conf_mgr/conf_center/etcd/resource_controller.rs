//! EtcdResourceController — per-kind event loop processing EtcdEvents.
//!
//! Mirrors `FileSystemResourceController` but:
//! - Receives `EtcdEvent` from bounded mpsc channel (backpressure, no event loss)
//! - Uses `EtcdStatusHandler` (async) for status persistence
//! - Worker reads from `LocalRawStore` (updated by event loop) instead of re-fetching from etcd

use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::task::JoinHandle;

use super::event::{EtcdDocument, EtcdEvent};
use super::status::EtcdStatusHandler;
use super::watcher::KindEventReceiver;
use crate::conf_mgr::conf_center::common::{
    clear_runtime_invalid, get_runtime_invalid, parse_resource_content_offline, store_runtime_invalid,
    RuntimeInvalidEnvelopes,
};
use crate::conf_mgr::sync_runtime::metrics::{controller_metrics, InitSyncTimer};
use crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;
use crate::conf_mgr::sync_runtime::resource_processor::{
    extract_status_value, make_resource_key, LocalRawStore, ResourceProcessor, WorkItemResult,
};
use crate::conf_mgr::sync_runtime::ShutdownSignal;
use edgion_resources::ResourceMeta;

/// EtcdResourceController — handles events for a single resource type.
pub struct EtcdResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    kind: &'static str,
    processor: Arc<ResourceProcessor<K>>,
    status_handler: Arc<EtcdStatusHandler>,
    event_rx: KindEventReceiver,
    conf_prefix: Arc<str>,
    shutdown_signal: Option<ShutdownSignal>,
    fs_raw_store: Arc<LocalRawStore<K>>,
    invalid_envelopes: Arc<RuntimeInvalidEnvelopes>,
}

impl<K> EtcdResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    pub fn new(
        kind: &'static str,
        processor: Arc<ResourceProcessor<K>>,
        status_handler: Arc<EtcdStatusHandler>,
        event_rx: KindEventReceiver,
        conf_prefix: Arc<str>,
    ) -> Self {
        let fs_raw_store = Arc::new(LocalRawStore::new());
        let raw_store: Arc<dyn RawStore<K>> = fs_raw_store.clone();
        processor.set_raw_store(raw_store.clone());
        crate::observe::status_metrics::register_status_renderer::<K>(kind, raw_store.clone());
        crate::observe::unknown_fields_registry::register_unknown_fields_source::<K>(kind, raw_store);
        Self {
            kind,
            processor,
            status_handler,
            event_rx,
            conf_prefix,
            shutdown_signal: None,
            fs_raw_store,
            invalid_envelopes: Arc::new(RuntimeInvalidEnvelopes::default()),
        }
    }

    pub fn with_shutdown(mut self, signal: ShutdownSignal) -> Self {
        self.shutdown_signal = Some(signal);
        self
    }

    /// Run the event loop.
    pub async fn run(mut self) -> Result<()> {
        let kind = self.kind;

        let _active = controller_metrics().active_controller();
        tracing::info!(
            component = "etcd_resource_controller",
            kind = kind,
            "Starting EtcdResourceController"
        );

        let mut init_done = false;
        let mut init_count: usize = 0;
        let mut init_timer: Option<InitSyncTimer> = None;
        let mut worker_handle: Option<JoinHandle<()>> = None;
        let mut shutdown = self.shutdown_signal.clone();

        loop {
            let event = if let Some(ref mut signal) = shutdown {
                tokio::select! {
                    event = self.event_rx.recv() => event,
                    _ = signal.wait() => {
                        tracing::info!(
                            component = "etcd_resource_controller",
                            kind = kind,
                            "Shutdown signal received"
                        );
                        break;
                    }
                }
            } else {
                self.event_rx.recv().await
            };

            match event {
                Some(event) => match event {
                    EtcdEvent::Init => {
                        if !init_done {
                            tracing::debug!(
                                component = "etcd_resource_controller",
                                kind = kind,
                                "Init phase started"
                            );
                            init_timer = Some(InitSyncTimer::start(kind));
                            self.processor.on_init();
                        }
                    }
                    EtcdEvent::InitApply { key, content } => match parse_etcd_document::<K>(&content) {
                        Ok(obj) => {
                            let content_key = make_resource_key(&obj);
                            let Some(rkey) =
                                validated_init_key(&self.status_handler, &self.conf_prefix, kind, &key, &content_key)
                                    .await
                            else {
                                continue;
                            };
                            self.fs_raw_store.upsert(rkey.clone(), Arc::new(obj.clone()));

                            let existing_status_json = self.status_handler.read_status_json(kind, &rkey).await;
                            // A resource repaired while the controller was down
                            // still carries its failure status; note it before
                            // the seed consumes it.
                            let had_error_sidecar = existing_status_json
                                .as_deref()
                                .is_some_and(crate::conf_mgr::conf_center::common::is_error_status_sidecar_json);

                            let result = self.processor.on_init_apply(obj, existing_status_json).await;

                            if let WorkItemResult::Processed {
                                obj,
                                status_changed,
                                persisted_status,
                            } = result
                            {
                                init_count += 1;
                                if status_changed {
                                    self.persist_status(kind, &rkey, &obj).await;
                                } else if persisted_status.is_none() && had_error_sidecar {
                                    self.clear_stale_error_status(kind, &rkey).await;
                                }
                            }
                        }
                        Err(e) => {
                            let rkey = extract_key_from_etcd_key(&key, &self.conf_prefix, kind);
                            if let Some(rkey) = rkey {
                                let _ = self
                                    .status_handler
                                    .write_error_status(kind, &rkey, "ParseError", &e.to_string())
                                    .await;
                            }
                            tracing::warn!(
                                component = "etcd_resource_controller",
                                kind = kind,
                                key = %key,
                                error = %e,
                                "Failed to parse resource during init"
                            );
                        }
                    },
                    EtcdEvent::InitDone => {
                        if init_done {
                            tracing::warn!(
                                component = "etcd_resource_controller",
                                kind = kind,
                                "Received duplicate InitDone event, ignoring"
                            );
                            continue;
                        }

                        let init_duration = init_timer.take().map(|t| t.complete(init_count)).unwrap_or(0.0);
                        tracing::info!(
                            component = "etcd_resource_controller",
                            kind = kind,
                            count = init_count,
                            duration_secs = init_duration,
                            "Init phase complete"
                        );

                        self.processor.on_init_done();
                        self.processor.notify_ready();
                        init_done = true;

                        worker_handle = Some(spawn_worker(
                            self.processor.clone(),
                            kind,
                            self.shutdown_signal.clone(),
                            self.fs_raw_store.clone(),
                            self.status_handler.clone(),
                            self.invalid_envelopes.clone(),
                        ));
                    }
                    EtcdEvent::Apply { key, content } => {
                        if !init_done {
                            // During init, treat as InitApply
                            if let Ok(obj) = parse_etcd_document::<K>(&content) {
                                let content_key = make_resource_key(&obj);
                                let Some(rkey) = validated_init_key(
                                    &self.status_handler,
                                    &self.conf_prefix,
                                    kind,
                                    &key,
                                    &content_key,
                                )
                                .await
                                else {
                                    continue;
                                };
                                self.fs_raw_store.upsert(rkey.clone(), Arc::new(obj.clone()));
                                let existing_status_json = self.status_handler.read_status_json(kind, &rkey).await;
                                let had_error_sidecar = existing_status_json
                                    .as_deref()
                                    .is_some_and(crate::conf_mgr::conf_center::common::is_error_status_sidecar_json);
                                let result = self.processor.on_init_apply(obj, existing_status_json).await;
                                if let WorkItemResult::Processed {
                                    obj,
                                    status_changed,
                                    persisted_status,
                                } = result
                                {
                                    init_count += 1;
                                    if status_changed {
                                        self.persist_status(kind, &rkey, &obj).await;
                                    } else if persisted_status.is_none() && had_error_sidecar {
                                        self.clear_stale_error_status(kind, &rkey).await;
                                    }
                                }
                            }
                        } else {
                            self.apply_runtime_document(&key, &content).await;
                        }
                    }
                    EtcdEvent::Delete { key } => {
                        if init_done {
                            self.delete_runtime_document(&key).await;
                        }
                    }
                },
                None => {
                    tracing::warn!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        "Event channel closed"
                    );
                    break;
                }
            }
        }

        // Wait for worker
        if let Some(handle) = worker_handle {
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        error = %e,
                        "Worker task panicked"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        "Worker task did not finish within 5 seconds"
                    );
                }
            }
        }

        tracing::info!(
            component = "etcd_resource_controller",
            kind = kind,
            "EtcdResourceController stopped"
        );
        Ok(())
    }

    async fn apply_runtime_document(&self, key: &str, content: &EtcdDocument) {
        let kind = self.kind;
        match parse_etcd_document::<K>(content) {
            Ok(obj) => {
                let content_key = make_resource_key(&obj);
                let rkey = match validate_etcd_identity(key, &self.conf_prefix, kind, &content_key) {
                    Ok(rkey) => rkey,
                    Err(EtcdIdentityError::Mismatch { key_identity }) => {
                        let message = identity_mismatch_message(&content_key, &key_identity);
                        store_runtime_invalid(
                            &self.invalid_envelopes,
                            key_identity.clone(),
                            "IdentityMismatch",
                            message,
                        );
                        self.fs_raw_store.remove(&key_identity);
                        log_identity_mismatch(kind, key, &content_key, &key_identity);
                        self.processor.on_event_key(key_identity).await;
                        return;
                    }
                    Err(EtcdIdentityError::MalformedKey) => {
                        log_malformed_etcd_key(kind, key);
                        return;
                    }
                };
                clear_runtime_invalid(&self.invalid_envelopes, &rkey);
                self.fs_raw_store.upsert(rkey.clone(), Arc::new(obj.clone()));
                self.processor.on_event_key(rkey).await;
            }
            Err(e) => {
                if let Some(rkey) = extract_key_from_etcd_key(key, &self.conf_prefix, kind) {
                    self.fs_raw_store.remove(&rkey);
                    store_runtime_invalid(&self.invalid_envelopes, rkey.clone(), "ParseError", e.to_string());
                    self.processor.on_event_key(rkey).await;
                }
                tracing::warn!(
                    component = "etcd_resource_controller",
                    kind = kind,
                    key = %key,
                    error = %e,
                    "Failed to parse resource for apply"
                );
            }
        }
    }

    async fn delete_runtime_document(&self, key: &str) {
        let kind = self.kind;
        if let Some(rkey) = extract_key_from_etcd_key(key, &self.conf_prefix, kind) {
            clear_runtime_invalid(&self.invalid_envelopes, &rkey);
            self.fs_raw_store.remove(&rkey);
            self.processor.on_event_key(rkey).await;
        } else {
            log_malformed_etcd_key(kind, key);
        }
    }

    /// Persist status to etcd.
    async fn persist_status(&self, kind: &str, key: &str, obj: &K) {
        if let Some(status_value) = extract_status_value(obj) {
            match self.status_handler.write_status_value(kind, key, &status_value).await {
                Ok(()) => {
                    crate::conf_mgr::sync_runtime::record_status_write(kind, "etcd");
                }
                Err(e) => {
                    crate::conf_mgr::sync_runtime::record_status_write_error(kind, "etcd");
                    tracing::warn!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        key = %key,
                        error = %e,
                        "Failed to write status to etcd"
                    );
                }
            }
        }
    }

    /// Drop the error status a previous failure left behind for a resource that
    /// computes no status of its own; nothing else would ever overwrite it.
    async fn clear_stale_error_status(&self, kind: &str, key: &str) {
        if let Err(e) = self.status_handler.delete_status(kind, key).await {
            tracing::warn!(
                component = "etcd_resource_controller",
                kind = kind,
                key = %key,
                error = %e,
                "Failed to clear stale error status after repair"
            );
        }
    }
}

/// Interpret the watcher outcome at the existing malformed-resource owner.
fn parse_etcd_document<K>(content: &EtcdDocument) -> Result<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    match content {
        EtcdDocument::Text(text) => Ok(parse_resource_content_offline(text)?),
        EtcdDocument::InvalidUtf8 => Err(anyhow::anyhow!(
            "etcd resource value is not valid UTF-8; replace it with UTF-8 YAML"
        )),
    }
}

/// Spawn worker task for processing workqueue items.
///
/// Unlike FileSystem mode which re-reads files, the etcd worker uses
/// the `LocalRawStore` which is always up-to-date from the event loop.
fn spawn_worker<K>(
    processor: Arc<ResourceProcessor<K>>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
    fs_raw_store: Arc<LocalRawStore<K>>,
    status_handler: Arc<EtcdStatusHandler>,
    invalid_envelopes: Arc<RuntimeInvalidEnvelopes>,
) -> JoinHandle<()>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    let workqueue = processor.workqueue();

    tokio::spawn(async move {
        let mut shutdown = shutdown_signal;

        loop {
            let item = match &mut shutdown {
                Some(signal) => {
                    tokio::select! {
                        item = workqueue.dequeue() => item,
                        _ = signal.wait() => break,
                    }
                }
                None => workqueue.dequeue().await,
            };

            match item {
                Some(work_item) => {
                    if let Some(invalid) = get_runtime_invalid(&invalid_envelopes, &work_item.key) {
                        let _ = processor
                            .process_work_item(&work_item.key, None, None, work_item.trigger_chain.clone())
                            .await;
                        if let Err(e) = status_handler
                            .write_error_status(kind, &work_item.key, invalid.reason, &invalid.message)
                            .await
                        {
                            tracing::warn!(
                                component = "etcd_resource_controller",
                                kind = kind,
                                key = %work_item.key,
                                error = %e,
                                "Failed to write runtime-invalid error status"
                            );
                        }
                        workqueue.done(&work_item.key);
                        continue;
                    }

                    // Get the latest object from raw_store (updated by event loop)
                    let store_obj = fs_raw_store.get(&work_item.key).map(|arc_obj| (*arc_obj).clone());

                    // Status read only when there is an object to process — in
                    // etcd mode this is a network RPC; deletes must not pay it.
                    let existing_status_json = if store_obj.is_some() {
                        status_handler.read_status_json(kind, &work_item.key).await
                    } else {
                        None
                    };
                    // A resource that produces no status of its own cannot
                    // overwrite the status a previous failure left behind, so
                    // remember whether one is present before it is consumed.
                    let had_error_sidecar = existing_status_json
                        .as_deref()
                        .is_some_and(crate::conf_mgr::conf_center::common::is_error_status_sidecar_json);

                    let result = processor
                        .process_work_item(
                            &work_item.key,
                            store_obj,
                            existing_status_json,
                            work_item.trigger_chain.clone(),
                        )
                        .await;

                    match result {
                        WorkItemResult::Processed {
                            obj,
                            status_changed,
                            persisted_status,
                        } => {
                            if status_changed {
                                if let Some(status_value) = extract_status_value(&obj) {
                                    match status_handler
                                        .write_status_value(kind, &work_item.key, &status_value)
                                        .await
                                    {
                                        Ok(()) => {
                                            crate::conf_mgr::sync_runtime::record_status_write(kind, "etcd");
                                        }
                                        Err(e) => {
                                            crate::conf_mgr::sync_runtime::record_status_write_error(kind, "etcd");
                                            tracing::warn!(
                                                component = "etcd_resource_controller",
                                                kind = kind,
                                                key = %work_item.key,
                                                error = %e,
                                                "Failed to write status to etcd"
                                            );
                                        }
                                    }
                                } else {
                                    tracing::warn!(
                                        component = "etcd_resource_controller",
                                        kind = kind,
                                        key = %work_item.key,
                                        "Status changed but failed to extract"
                                    );
                                }
                            } else if persisted_status.is_none() && had_error_sidecar {
                                // A repaired resource that computes no status of
                                // its own must still drop the retained error
                                // status; nothing else will overwrite it.
                                if let Err(e) = status_handler.delete_status(kind, &work_item.key).await {
                                    tracing::warn!(
                                        component = "etcd_resource_controller",
                                        kind = kind,
                                        key = %work_item.key,
                                        error = %e,
                                        "Failed to clear stale error status after repair"
                                    );
                                }
                            }
                        }
                        WorkItemResult::Deleted { key } => {
                            let _ = status_handler.delete_status(kind, &key).await;
                        }
                        WorkItemResult::Skipped => {}
                    }

                    workqueue.done(&work_item.key);
                }
                None => break,
            }
        }

        tracing::info!(component = "etcd_resource_controller", kind = kind, "Worker task ended");
    })
}

/// Extract resource key ("namespace/name" or "name") from etcd key.
///
/// Key format: `{prefix}/conf/{kind}/{namespace}/{name}`
/// e.g. "/edgion/conf/HTTPRoute/default/my-route" → "default/my-route"
///      "/edgion/conf/GatewayClass/_/edgion"       → "edgion"
fn extract_key_from_etcd_key(etcd_key: &str, conf_prefix: &str, expected_kind: &str) -> Option<String> {
    let relative = etcd_key.strip_prefix(conf_prefix)?;
    let (kind, ns_and_name) = relative.split_once('/')?;
    if kind != expected_kind {
        return None;
    }
    let (ns, name) = ns_and_name.split_once('/')?;
    if ns.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    if ns == "_" {
        Some(name.to_string())
    } else {
        Some(format!("{}/{}", ns, name))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EtcdIdentityError {
    MalformedKey,
    Mismatch { key_identity: String },
}

/// Return the authoritative key identity only when the etcd key is strictly
/// formed and agrees with the resource content.
fn validate_etcd_identity(
    etcd_key: &str,
    conf_prefix: &str,
    expected_kind: &str,
    content_key: &str,
) -> Result<String, EtcdIdentityError> {
    let key_identity =
        extract_key_from_etcd_key(etcd_key, conf_prefix, expected_kind).ok_or(EtcdIdentityError::MalformedKey)?;
    if key_identity != content_key {
        return Err(EtcdIdentityError::Mismatch { key_identity });
    }
    Ok(key_identity)
}

fn identity_mismatch_message(content_key: &str, key_identity: &str) -> String {
    format!(
        "resource content identity '{content_key}' does not match etcd key identity '{key_identity}'; \
         in etcd mode the key is authoritative — move the value to the matching key or revert \
         metadata.name/namespace to '{key_identity}'"
    )
}

fn log_identity_mismatch(kind: &str, etcd_key: &str, content_key: &str, key_identity: &str) {
    tracing::warn!(
        component = "etcd_resource_controller",
        kind = kind,
        key = %etcd_key,
        content_key = %content_key,
        key_identity = %key_identity,
        "Rejected resource: content identity does not match etcd key identity"
    );
}

fn log_malformed_etcd_key(kind: &str, etcd_key: &str) {
    tracing::warn!(
        component = "etcd_resource_controller",
        kind = kind,
        key = %etcd_key,
        "Rejected resource: malformed etcd resource key"
    );
}

async fn validated_init_key(
    status_handler: &EtcdStatusHandler,
    conf_prefix: &str,
    kind: &str,
    etcd_key: &str,
    content_key: &str,
) -> Option<String> {
    match validate_etcd_identity(etcd_key, conf_prefix, kind, content_key) {
        Ok(key_identity) => Some(key_identity),
        Err(EtcdIdentityError::Mismatch { key_identity }) => {
            reject_identity_mismatch(status_handler, kind, etcd_key, content_key, &key_identity).await;
            None
        }
        Err(EtcdIdentityError::MalformedKey) => {
            log_malformed_etcd_key(kind, etcd_key);
            None
        }
    }
}

/// Reject content whose metadata identity disagrees with its authoritative
/// etcd key and persist the diagnostic under that key's identity.
async fn reject_identity_mismatch(
    status_handler: &EtcdStatusHandler,
    kind: &str,
    etcd_key: &str,
    content_key: &str,
    key_identity: &str,
) {
    let message = identity_mismatch_message(content_key, key_identity);
    if let Err(e) = status_handler
        .write_error_status(kind, key_identity, "IdentityMismatch", &message)
        .await
    {
        tracing::warn!(
            component = "etcd_resource_controller",
            kind = kind,
            key = %etcd_key,
            error = %e,
            "Failed to write identity-mismatch error status"
        );
    }
    log_identity_mismatch(kind, etcd_key, content_key, key_identity);
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/etcd/resource_controller_test.rs"]
mod tests;
