//! FileSystemResourceController - Per-resource controller for FileSystem mode
//!
//! This controller receives events from FileSystemWatcher's bounded channel
//! and processes them using the same logic as Kubernetes mode.
//!
//! The event handling logic mirrors K8s mode's ResourceController:
//! - Init -> processor.on_init()
//! - InitApply { path, content } -> parse + processor.on_init_apply(obj)
//! - InitDone -> processor.on_init_done()
//! - Apply { path, content } -> parse + processor.on_event_key(key)
//! - Delete(info) -> processor.on_event_key(key)

use super::event::FileSystemEvent;
use super::file_watcher::KindEventReceiver;
use super::path_registry::{PathRegistry, ResourceKey};
use super::status::FileSystemStatusHandler;
use super::storage::build_resource_path_from_key;
#[cfg(test)]
use crate::conf_mgr::conf_center::common::parse_resource_content;
use crate::conf_mgr::conf_center::common::{
    clear_runtime_invalid, get_runtime_invalid, parse_resource_content_offline, store_runtime_invalid,
    RuntimeInvalidEnvelopes,
};
use crate::conf_mgr::conf_center::non_kube_common::LocalTaskScope;
use crate::conf_mgr::sync_runtime::metrics::{controller_metrics, InitSyncTimer};
use crate::conf_mgr::sync_runtime::resource_processor::{
    extract_status_value, make_resource_key, LocalRawStore, ResourceProcessor, WorkItemResult,
};
use crate::conf_mgr::sync_runtime::ShutdownSignal;
use anyhow::Result;
use edgion_resources::ResourceMeta;
use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::task::AbortOnDropHandle;

/// FileSystemResourceController - handles events for a single resource type
pub struct FileSystemResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    kind: &'static str,
    processor: Arc<ResourceProcessor<K>>,
    conf_dir: PathBuf,
    /// Shared with watcher / status handler / storage — single source of truth
    /// for `(kind, key) → on-disk path`. Used here to resolve the spec path
    /// the worker re-reads (may be nested) and to feed the status handler so
    /// `.yaml.status` lands beside its spec.
    registry: Arc<PathRegistry>,
    event_rx: KindEventReceiver,
    shutdown_signal: Option<ShutdownSignal>,
    scope: LocalTaskScope,
    /// Raw store shared with processor for cross-resource key lookups.
    fs_raw_store: Arc<LocalRawStore<K>>,
    invalid_envelopes: Arc<RuntimeInvalidEnvelopes>,
}

impl<K> FileSystemResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    /// Create a new FileSystemResourceController
    pub fn new(
        kind: &'static str,
        processor: Arc<ResourceProcessor<K>>,
        conf_dir: PathBuf,
        registry: Arc<PathRegistry>,
        event_rx: KindEventReceiver,
    ) -> Self {
        let fs_raw_store = Arc::new(LocalRawStore::new());
        let raw_store: std::sync::Arc<dyn crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore<K>> =
            fs_raw_store.clone();
        processor.set_raw_store(raw_store.clone());
        crate::observe::status_metrics::register_status_renderer::<K>(kind, raw_store.clone());
        crate::observe::unknown_fields_registry::register_unknown_fields_source::<K>(kind, raw_store);
        Self {
            kind,
            processor,
            conf_dir,
            registry,
            event_rx,
            shutdown_signal: None,
            scope: LocalTaskScope::default(),
            fs_raw_store,
            invalid_envelopes: Arc::new(RuntimeInvalidEnvelopes::default()),
        }
    }

    /// Set shutdown signal
    pub fn with_shutdown(mut self, signal: ShutdownSignal) -> Self {
        self.shutdown_signal = Some(signal);
        self
    }

    pub(super) fn with_scope(mut self, scope: LocalTaskScope) -> Self {
        self.scope = scope;
        self
    }

    /// Run the controller event loop
    pub async fn run(mut self) -> Result<()> {
        let kind = self.kind;

        let _active = controller_metrics().active_controller();
        tracing::info!(
            component = "fs_resource_controller",
            kind = kind,
            "Starting FileSystemResourceController"
        );

        let mut init_done = false;
        let mut init_count: usize = 0;
        let mut init_timer: Option<InitSyncTimer> = None;
        let mut worker_handle: Option<AbortOnDropHandle<()>> = None;
        let mut shutdown = self.shutdown_signal.clone();

        // Status handler for init phase. Registry-aware so `.yaml.status` writes
        // land beside the spec (which may be nested under a namespace subdir).
        let status_handler = FileSystemStatusHandler::new(self.conf_dir.clone()).with_registry(self.registry.clone());

        loop {
            let event = if let Some(ref mut signal) = shutdown {
                tokio::select! {
                    event = self.event_rx.recv() => event,
                    _ = signal.wait() => {
                        tracing::info!(
                            component = "fs_resource_controller",
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
                Some(event) => {
                    match event {
                        FileSystemEvent::Init => {
                            if init_done {
                                // Spurious Init after InitDone — log and ignore.
                                tracing::warn!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    "Received Init event after init done, ignoring"
                                );
                            } else {
                                tracing::debug!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    "Init phase started"
                                );
                                init_timer = Some(InitSyncTimer::start(kind));
                                self.processor.on_init();
                            }
                        }
                        FileSystemEvent::InitApply { path, content } => {
                            // Parse YAML content to resource type
                            match parse_resource_content_offline::<K>(&content) {
                                Ok(obj) => {
                                    let key = make_resource_key(&obj);

                                    // Reject files whose content identity drifted from their
                                    // filename to avoid registry/work-key divergence.
                                    if let Some(filename_key) = filename_identity_mismatch(&path, kind, &key) {
                                        reject_identity_mismatch(&status_handler, kind, &path, &key, &filename_key)
                                            .await;
                                        continue;
                                    }

                                    // Maintain raw store for cross-resource lookups.
                                    self.fs_raw_store.upsert(key.clone(), Arc::new(obj.clone()));

                                    // Read existing status from .status file for comparison
                                    let existing_status_json = status_handler.read_status_json(kind, &key).await;
                                    // A file repaired while the controller was
                                    // down still carries its failure sidecar;
                                    // note it before the seed consumes it.
                                    let had_error_sidecar = existing_status_json.as_deref().is_some_and(
                                        crate::conf_mgr::conf_center::common::is_error_status_sidecar_json,
                                    );

                                    let result = self.processor.on_init_apply(obj, existing_status_json).await;

                                    // Handle status persistence (same as runtime)
                                    match result {
                                        WorkItemResult::Processed {
                                            obj,
                                            status_changed,
                                            persisted_status,
                                        } => {
                                            init_count += 1;
                                            if status_changed {
                                                match extract_status_value(&obj) {
                                                    Some(status_value) => {
                                                        match status_handler
                                                            .write_status_value(kind, &key, &status_value)
                                                            .await
                                                        {
                                                            Ok(()) => {
                                                                crate::conf_mgr::sync_runtime::record_status_write(
                                                                    kind,
                                                                    "filesystem",
                                                                );
                                                                tracing::info!(
                                                                    component = "fs_resource_controller",
                                                                    kind = kind,
                                                                    key = %key,
                                                                    "Persisted status to filesystem during init"
                                                                );
                                                            }
                                                            Err(e) => {
                                                                crate::conf_mgr::sync_runtime::record_status_write_error(kind, "filesystem");
                                                                tracing::warn!(
                                                                    component = "fs_resource_controller",
                                                                    kind = kind,
                                                                    key = %key,
                                                                    error = %e,
                                                                    "Failed to write status file during init"
                                                                );
                                                            }
                                                        }
                                                    }
                                                    None => {
                                                        tracing::warn!(
                                                            component = "fs_resource_controller",
                                                            kind = kind,
                                                            key = %key,
                                                            "Status changed but failed to extract during init"
                                                        );
                                                    }
                                                }
                                            } else if persisted_status.is_none() && had_error_sidecar {
                                                // A repaired resource that computes no
                                                // status of its own must still drop the
                                                // retained error sidecar.
                                                if let Err(e) = status_handler.delete_status(kind, &key).await {
                                                    tracing::warn!(
                                                        component = "fs_resource_controller",
                                                        kind = kind,
                                                        key = %key,
                                                        error = %e,
                                                        "Failed to clear stale error status during init"
                                                    );
                                                }
                                            }
                                        }
                                        WorkItemResult::Skipped | WorkItemResult::Deleted { .. } => {}
                                    }
                                }
                                Err(e) => {
                                    // Write error status for parse failures
                                    let error_msg = e.to_string();
                                    // Extract key from path
                                    if let Some(key) = extract_key_from_path(&path, kind) {
                                        if let Err(write_err) = status_handler
                                            .write_error_status(kind, &key, "ParseError", &error_msg)
                                            .await
                                        {
                                            tracing::warn!(
                                                component = "fs_resource_controller",
                                                kind = kind,
                                                path = %path.display(),
                                                error = %write_err,
                                                "Failed to write error status during init"
                                            );
                                        }
                                    }
                                    tracing::warn!(
                                        component = "fs_resource_controller",
                                        kind = kind,
                                        path = %path.display(),
                                        error = %error_msg,
                                        "Failed to parse resource during init"
                                    );
                                }
                            }
                        }
                        FileSystemEvent::InitDone => {
                            if init_done {
                                tracing::warn!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    "Received duplicate InitDone event, ignoring"
                                );
                                continue;
                            }

                            let init_duration = init_timer.take().map(|t| t.complete(init_count)).unwrap_or(0.0);
                            tracing::info!(
                                component = "fs_resource_controller",
                                kind = kind,
                                count = init_count,
                                duration_secs = init_duration,
                                "Init phase complete"
                            );

                            // Mark cache ready
                            self.processor.on_init_done();
                            self.processor.notify_ready();
                            init_done = true;

                            // Spawn worker for runtime phase
                            worker_handle = Some(AbortOnDropHandle::new(spawn_worker(
                                self.processor.clone(),
                                self.conf_dir.clone(),
                                self.registry.clone(),
                                kind,
                                self.shutdown_signal.clone(),
                                self.fs_raw_store.clone(),
                                self.invalid_envelopes.clone(),
                                &self.scope,
                            )));

                            tracing::info!(
                                component = "fs_resource_controller",
                                kind = kind,
                                "Worker started, processing runtime events"
                            );
                        }
                        FileSystemEvent::Apply { path, content } => {
                            if !init_done {
                                // During init phase, treat as InitApply
                                match parse_resource_content_offline::<K>(&content) {
                                    Ok(obj) => {
                                        let key = make_resource_key(&obj);
                                        if let Some(filename_key) = filename_identity_mismatch(&path, kind, &key) {
                                            self.fs_raw_store.remove(&filename_key);
                                            store_runtime_invalid(
                                                &self.invalid_envelopes,
                                                filename_key.clone(),
                                                "IdentityMismatch",
                                                identity_mismatch_message(&key, &filename_key),
                                            );
                                            tracing::warn!(
                                                component = "fs_resource_controller",
                                                kind = kind,
                                                path = %path.display(),
                                                content_key = %key,
                                                filename_key = %filename_key,
                                                "Rejected resource: file content identity does not match filename identity"
                                            );
                                            self.processor.on_event_key(filename_key).await;
                                            continue;
                                        }
                                        self.fs_raw_store.upsert(key.clone(), Arc::new(obj.clone()));
                                        let existing_status_json = status_handler.read_status_json(kind, &key).await;
                                        let had_error_sidecar = existing_status_json.as_deref().is_some_and(
                                            crate::conf_mgr::conf_center::common::is_error_status_sidecar_json,
                                        );
                                        let result = self.processor.on_init_apply(obj, existing_status_json).await;

                                        if let WorkItemResult::Processed {
                                            obj,
                                            status_changed,
                                            persisted_status,
                                        } = result
                                        {
                                            init_count += 1;
                                            if status_changed {
                                                if let Some(status_value) = extract_status_value(&obj) {
                                                    match status_handler
                                                        .write_status_value(kind, &key, &status_value)
                                                        .await
                                                    {
                                                        Ok(()) => {
                                                            crate::conf_mgr::sync_runtime::record_status_write(
                                                                kind,
                                                                "filesystem",
                                                            );
                                                            tracing::info!(
                                                                component = "fs_resource_controller",
                                                                kind = kind,
                                                                key = %key,
                                                                "Persisted status to filesystem during apply-as-init"
                                                            );
                                                        }
                                                        Err(e) => {
                                                            crate::conf_mgr::sync_runtime::record_status_write_error(
                                                                kind,
                                                                "filesystem",
                                                            );
                                                            tracing::warn!(
                                                                component = "fs_resource_controller",
                                                                kind = kind,
                                                                key = %key,
                                                                error = %e,
                                                                "Failed to write status file during apply-as-init"
                                                            );
                                                        }
                                                    }
                                                }
                                            } else if persisted_status.is_none() && had_error_sidecar {
                                                // A repaired resource that computes no
                                                // status of its own must still drop the
                                                // retained error sidecar.
                                                if let Err(e) = status_handler.delete_status(kind, &key).await {
                                                    tracing::warn!(
                                                        component = "fs_resource_controller",
                                                        kind = kind,
                                                        key = %key,
                                                        error = %e,
                                                        "Failed to clear stale error status during apply-as-init"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            path = %path.display(),
                                            error = %e,
                                            "Failed to parse resource during apply"
                                        );
                                    }
                                }
                            } else {
                                // Runtime phase - parse and enqueue
                                match parse_resource_content_offline::<K>(&content) {
                                    Ok(obj) => {
                                        let key = make_resource_key(&obj);
                                        if let Some(filename_key) = filename_identity_mismatch(&path, kind, &key) {
                                            // The filename identity is authoritative in filesystem mode.
                                            // Publish the invalid envelope and enqueue that same key so the
                                            // sole worker revokes the cached generation and all derived state
                                            // before retaining the error status. A later repaired Apply clears
                                            // the envelope and follows this same queue.
                                            self.fs_raw_store.remove(&filename_key);
                                            store_runtime_invalid(
                                                &self.invalid_envelopes,
                                                filename_key.clone(),
                                                "IdentityMismatch",
                                                identity_mismatch_message(&key, &filename_key),
                                            );
                                            tracing::warn!(
                                                component = "fs_resource_controller",
                                                kind = kind,
                                                path = %path.display(),
                                                content_key = %key,
                                                filename_key = %filename_key,
                                                "Rejected runtime resource with mismatched file identity; enqueuing authoritative tombstone"
                                            );
                                            self.processor.on_event_key(filename_key).await;
                                            continue;
                                        }
                                        tracing::info!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            key = %key,
                                            path = %path.display(),
                                            "Received runtime Apply event, enqueuing"
                                        );
                                        clear_runtime_invalid(&self.invalid_envelopes, &key);
                                        self.fs_raw_store.upsert(key.clone(), Arc::new(obj.clone()));
                                        self.processor.on_event_key(key).await;
                                    }
                                    Err(e) => {
                                        // File content is invalid — evict any stale entry from
                                        // LocalRawStore and enqueue the authoritative key. The
                                        // single worker observes the tombstone and revokes the
                                        // previously processed generation and all of its derived
                                        // state. A later valid apply follows the same queue path.
                                        if let Some(key) = extract_key_from_path(&path, kind) {
                                            self.fs_raw_store.remove(&key);
                                            store_runtime_invalid(
                                                &self.invalid_envelopes,
                                                key.clone(),
                                                "ParseError",
                                                e.to_string(),
                                            );
                                            self.processor.on_event_key(key).await;
                                        }
                                        tracing::warn!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            path = %path.display(),
                                            error = %e,
                                            "Failed to parse resource for apply, evicted from raw store"
                                        );
                                    }
                                }
                            }
                        }
                        FileSystemEvent::Delete(info) => {
                            if !init_done {
                                tracing::warn!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    "Received Delete event during init phase, ignoring"
                                );
                            } else {
                                // Parse delete info: "__DELETE__:kind:key"
                                if let Some(key) = parse_delete_info(&info) {
                                    clear_runtime_invalid(&self.invalid_envelopes, &key);
                                    // Remove from raw store immediately on delete.
                                    self.fs_raw_store.remove(&key);
                                    // Enqueue unconditionally: the worker re-reads the
                                    // store and decides; a key absent from both store
                                    // and cache resolves to Skipped.
                                    self.processor.on_event_key(key).await;
                                }
                            }
                        }
                    }
                }
                None => {
                    if !init_done {
                        anyhow::bail!("FS initialization channel closed before completion for kind {kind}");
                    }
                    tracing::warn!(
                        component = "fs_resource_controller",
                        kind = kind,
                        "Event channel closed"
                    );
                    break;
                }
            }
        }

        // Wait for worker task to finish
        if let Some(mut handle) = worker_handle {
            tracing::info!(
                component = "fs_resource_controller",
                kind = kind,
                "Waiting for worker task to finish..."
            );

            match tokio::time::timeout(Duration::from_secs(5), &mut handle).await {
                Ok(Ok(())) => {
                    tracing::info!(
                        component = "fs_resource_controller",
                        kind = kind,
                        "Worker task finished gracefully"
                    );
                }
                Ok(Err(e)) => {
                    tracing::error!(
                        component = "fs_resource_controller",
                        kind = kind,
                        error = %e,
                        "Worker task panicked"
                    );
                }
                Err(_) => {
                    handle.abort();
                    let _ = handle.await;
                    tracing::warn!(
                        component = "fs_resource_controller",
                        kind = kind,
                        "Worker task did not finish within 5 seconds"
                    );
                }
            }
        }

        tracing::info!(
            component = "fs_resource_controller",
            kind = kind,
            "FileSystemResourceController stopped"
        );

        Ok(())
    }
}

/// Spawn worker task for processing workqueue items
#[allow(clippy::too_many_arguments)]
fn spawn_worker<K>(
    processor: Arc<ResourceProcessor<K>>,
    conf_dir: PathBuf,
    registry: Arc<PathRegistry>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
    fs_raw_store: Arc<LocalRawStore<K>>,
    invalid_envelopes: Arc<RuntimeInvalidEnvelopes>,
    scope: &LocalTaskScope,
) -> JoinHandle<()>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    let workqueue = processor.workqueue();
    let status_handler = FileSystemStatusHandler::new(conf_dir.clone()).with_registry(registry.clone());

    let cancel = scope.cancel.clone();
    scope.tracker.spawn(async move {
        let processing = async {
            // Clone shutdown_signal once outside the loop
            let mut shutdown = shutdown_signal;

            loop {
                let item = match &mut shutdown {
                    Some(signal) => {
                        tokio::select! {
                            item = workqueue.dequeue() => item,
                            _ = signal.wait() => {
                                tracing::info!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    "Worker received shutdown signal"
                                );
                                break;
                            }
                        }
                    }
                    None => workqueue.dequeue().await,
                };

                match item {
                    Some(work_item) => {
                        if let Some(invalid) = get_runtime_invalid(&invalid_envelopes, &work_item.key) {
                            // The sole worker owns both revocation and status
                            // publication. Do not route the Deleted result through
                            // the ordinary delete-status arm: ParseError must stay
                            // visible until a valid repair clears the envelope.
                            let _ = processor
                                .process_work_item(&work_item.key, None, None, work_item.trigger_chain.clone())
                                .await;
                            if let Err(e) = status_handler
                                .write_error_status(kind, &work_item.key, invalid.reason, &invalid.message)
                                .await
                            {
                                tracing::warn!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    key = %work_item.key,
                                    error = %e,
                                    "Failed to persist invalid-resource status"
                                );
                            }
                            workqueue.done(&work_item.key);
                            continue;
                        }
                        // For FileSystem mode we re-read the spec from disk. The
                        // canonical path lives in `PathRegistry` (winner of this
                        // (kind, key)); we fall back to the default nested layout
                        // only when the registry has no record — which mainly
                        // happens for keys that were enqueued but whose Create
                        // event the watcher hasn't yet observed.
                        let rkey = ResourceKey::new(kind.to_string(), work_item.key.clone());
                        let path = registry
                            .winner_of(&rkey)
                            .unwrap_or_else(|| build_resource_path_from_key(&conf_dir, kind, &work_item.key));

                        let (store_obj, parse_error) = if path.exists() {
                            match tokio::fs::read_to_string(&path).await {
                                Ok(content) => match parse_resource_content_offline::<K>(&content) {
                                    Ok(obj) => {
                                        // Guard the same content/filename identity invariant the
                                        // main loop enforces. The worker re-reads the registry-winner
                                        // file for `work_item.key`, but the file content could have
                                        // drifted to a different metadata.name/namespace between enqueue
                                        // and dequeue (a narrow race the main-loop reject can't cover for
                                        // an already-queued item). Applying it under the stale work key
                                        // would reintroduce the drift, so reject and surface an error
                                        // status instead.
                                        let content_key = make_resource_key(&obj);
                                        if content_key != work_item.key {
                                            let message = format!(
                                                "file content identity '{content_key}' does not match work key \
                                             '{}'; in FileSystem mode the filename is authoritative — rename \
                                             the file or revert metadata.name/namespace to '{}'",
                                                work_item.key, work_item.key
                                            );
                                            fs_raw_store.remove(&work_item.key);
                                            store_runtime_invalid(
                                                &invalid_envelopes,
                                                work_item.key.clone(),
                                                "IdentityMismatch",
                                                message.clone(),
                                            );
                                            let _ = processor
                                                .process_work_item(
                                                    &work_item.key,
                                                    None,
                                                    None,
                                                    work_item.trigger_chain.clone(),
                                                )
                                                .await;
                                            if let Err(e) = status_handler
                                                .write_error_status(kind, &work_item.key, "IdentityMismatch", &message)
                                                .await
                                            {
                                                tracing::warn!(
                                                    component = "fs_resource_controller",
                                                    kind = kind,
                                                    key = %work_item.key,
                                                    error = %e,
                                                    "Failed to write identity-mismatch error status"
                                                );
                                            }
                                            tracing::warn!(
                                                component = "fs_resource_controller",
                                                kind = kind,
                                                key = %work_item.key,
                                                content_key = %content_key,
                                                "Worker rejected resource: content identity does not match work key"
                                            );
                                            workqueue.done(&work_item.key);
                                            continue;
                                        }
                                        // Keep raw store in sync with the latest file version.
                                        // The main loop upserts on Apply events, but the worker
                                        // re-reads the file (possibly a newer version), so we
                                        // update here to stay consistent.
                                        fs_raw_store.upsert(work_item.key.clone(), Arc::new(obj.clone()));
                                        clear_runtime_invalid(&invalid_envelopes, &work_item.key);
                                        (Some(obj), None)
                                    }
                                    Err(e) => {
                                        let error_msg = e.to_string();
                                        fs_raw_store.remove(&work_item.key);
                                        store_runtime_invalid(
                                            &invalid_envelopes,
                                            work_item.key.clone(),
                                            "ParseError",
                                            error_msg.clone(),
                                        );
                                        tracing::warn!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            key = %work_item.key,
                                            error = %error_msg,
                                            "Failed to parse file"
                                        );
                                        (None, Some(error_msg))
                                    }
                                },
                                Err(e) => {
                                    fs_raw_store.remove(&work_item.key);
                                    let error_msg = e.to_string();
                                    tracing::warn!(
                                        component = "fs_resource_controller",
                                        kind = kind,
                                        key = %work_item.key,
                                        error = %error_msg,
                                        "Failed to read file"
                                    );
                                    (None, Some(error_msg))
                                }
                            }
                        } else {
                            // File no longer exists — resource was deleted.
                            // Remove from raw store in case the main-loop Delete event
                            // was missed (e.g. channel lagged).
                            fs_raw_store.remove(&work_item.key);
                            (None, None)
                        };

                        // Handle parse errors by writing error status
                        if let Some(error_msg) = parse_error {
                            let _ = processor
                                .process_work_item(&work_item.key, None, None, work_item.trigger_chain.clone())
                                .await;
                            if let Err(e) = status_handler
                                .write_error_status(kind, &work_item.key, "ParseError", &error_msg)
                                .await
                            {
                                tracing::warn!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    key = %work_item.key,
                                    error = %e,
                                    "Failed to write error status"
                                );
                            }
                            workqueue.done(&work_item.key);
                            continue;
                        }

                        // Read existing status only when there is a store object to
                        // process; the delete/skip arms never consume it, and a
                        // delete must not pay a status read.
                        let existing_status_json = if store_obj.is_some() {
                            status_handler.read_status_json(kind, &work_item.key).await
                        } else {
                            None
                        };
                        // A resource that produces no status of its own cannot
                        // overwrite the sidecar a previous failure left behind, so
                        // remember whether one is present before it is consumed.
                        let had_error_sidecar = existing_status_json
                            .as_deref()
                            .is_some_and(crate::conf_mgr::conf_center::common::is_error_status_sidecar_json);

                        // Use processor's process_work_item which handles the reconciliation logic
                        let result = processor
                            .process_work_item(
                                &work_item.key,
                                store_obj,
                                existing_status_json,
                                work_item.trigger_chain.clone(),
                            )
                            .await;

                        // Persist status based on result
                        match result {
                            WorkItemResult::Processed {
                                obj,
                                status_changed,
                                persisted_status,
                            } => {
                                if status_changed {
                                    match extract_status_value(&obj) {
                                        Some(status_value) => {
                                            match status_handler
                                                .write_status_value(kind, &work_item.key, &status_value)
                                                .await
                                            {
                                                Ok(()) => {
                                                    crate::conf_mgr::sync_runtime::record_status_write(
                                                        kind,
                                                        "filesystem",
                                                    );
                                                    tracing::info!(
                                                        component = "fs_resource_controller",
                                                        kind = kind,
                                                        key = %work_item.key,
                                                        "Persisted status to filesystem"
                                                    );
                                                }
                                                Err(e) => {
                                                    crate::conf_mgr::sync_runtime::record_status_write_error(
                                                        kind,
                                                        "filesystem",
                                                    );
                                                    tracing::warn!(
                                                        component = "fs_resource_controller",
                                                        kind = kind,
                                                        key = %work_item.key,
                                                        error = %e,
                                                        "Failed to write status file"
                                                    );
                                                }
                                            }
                                        }
                                        None => {
                                            tracing::warn!(
                                                component = "fs_resource_controller",
                                                kind = kind,
                                                key = %work_item.key,
                                                "Status changed but failed to extract"
                                            );
                                        }
                                    }
                                } else if persisted_status.is_none() && had_error_sidecar {
                                    // A repaired resource that computes no status of
                                    // its own must still drop the retained error
                                    // sidecar; nothing else will overwrite it.
                                    if let Err(e) = status_handler.delete_status(kind, &work_item.key).await {
                                        tracing::warn!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            key = %work_item.key,
                                            error = %e,
                                            "Failed to clear stale error status after repair"
                                        );
                                    }
                                }
                            }
                            WorkItemResult::Deleted { key } => {
                                // Delete status file when resource is deleted
                                if let Err(e) = status_handler.delete_status(kind, &key).await {
                                    tracing::warn!(
                                        component = "fs_resource_controller",
                                        kind = kind,
                                        key = %key,
                                        error = %e,
                                        "Failed to delete status file"
                                    );
                                }
                            }
                            WorkItemResult::Skipped => {
                                // Nothing to do
                            }
                        }

                        workqueue.done(&work_item.key);
                    }
                    None => {
                        tracing::warn!(
                            component = "fs_resource_controller",
                            kind = kind,
                            "Workqueue closed, stopping worker"
                        );
                        break;
                    }
                }
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            _ = processing => {},
        }
        tracing::info!(component = "fs_resource_controller", kind = kind, "Worker task ended");
    })
}

/// Parse delete info string: "__DELETE__:kind:key"
fn parse_delete_info(info: &str) -> Option<String> {
    if info.starts_with("__DELETE__:") {
        let parts: Vec<&str> = info.splitn(3, ':').collect();
        if parts.len() == 3 {
            return Some(parts[2].to_string());
        }
    }
    None
}

/// Detect a drift between the resource identity encoded in the on-disk
/// filename and the identity declared in the file content.
///
/// In FileSystem mode the filename (`{Kind}_{ns}_{name}.yaml`) is the
/// authoritative identity: the watcher derives the `PathRegistry` key from it,
/// while the work key the controller enqueues comes from the file content's
/// `metadata.name`/`namespace`. If an operator edits the content identity
/// without renaming the file, the registry key and the work key drift apart —
/// the resource is never reconciled under its new content key (the registry
/// has no winner for it, so the worker falls back to a non-existent path and
/// treats it as deleted) and the old key is left stale. Callers reject such a
/// file and surface an error status instead of silently drifting.
///
/// Returns the filename-derived key when it disagrees with `content_key`
/// (a mismatch that must be rejected); `None` when they agree or the filename
/// cannot be parsed (unparseable filenames are already handled elsewhere).
fn filename_identity_mismatch(path: &std::path::Path, kind: &str, content_key: &str) -> Option<String> {
    match extract_key_from_path(path, kind) {
        Some(filename_key) if filename_key != content_key => Some(filename_key),
        _ => None,
    }
}

/// Write an `IdentityMismatch` error status beside the offending file and log
/// the rejection. Keyed by the filename identity so the `.status` sidecar lands
/// next to the spec file (the registry winner for the filename key).
async fn reject_identity_mismatch(
    status_handler: &FileSystemStatusHandler,
    kind: &str,
    path: &std::path::Path,
    content_key: &str,
    filename_key: &str,
) {
    let message = identity_mismatch_message(content_key, filename_key);
    if let Err(e) = status_handler
        .write_error_status(kind, filename_key, "IdentityMismatch", &message)
        .await
    {
        tracing::warn!(
            component = "fs_resource_controller",
            kind = kind,
            path = %path.display(),
            error = %e,
            "Failed to write identity-mismatch error status"
        );
    }
    tracing::warn!(
        component = "fs_resource_controller",
        kind = kind,
        path = %path.display(),
        content_key = %content_key,
        filename_key = %filename_key,
        "Rejected resource: file content identity does not match filename identity"
    );
}

fn identity_mismatch_message(content_key: &str, filename_key: &str) -> String {
    format!(
        "file content identity '{content_key}' does not match filename identity '{filename_key}'; \
         in FileSystem mode the filename is authoritative — rename the file or revert \
         metadata.name/namespace to '{filename_key}'"
    )
}

/// Extract resource key from file path
///
/// Filename format:
/// - With namespace: `{Kind}_{namespace}_{name}.yaml` -> "namespace/name"
/// - Cluster-scoped: `{Kind}__{name}.yaml` -> "name"
fn extract_key_from_path(path: &std::path::Path, expected_kind: &str) -> Option<String> {
    let filename = path.file_stem()?.to_str()?;

    // Split into at most 3 parts by underscore
    let parts: Vec<&str> = filename.splitn(3, '_').collect();

    if parts.len() == 3 && parts[0] == expected_kind {
        let namespace = if parts[1].is_empty() {
            None // Double underscore means cluster-scoped
        } else {
            Some(parts[1])
        };
        let name = parts[2];

        Some(match namespace {
            Some(ns) => format!("{}/{}", ns, name),
            None => name.to_string(),
        })
    } else {
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/file_system/resource_controller_test.rs"]
mod tests;
