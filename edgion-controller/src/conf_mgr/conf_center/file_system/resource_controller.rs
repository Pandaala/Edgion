//! FileSystemResourceController - Per-resource controller for FileSystem mode
//!
//! This controller receives events from FileSystemWatcher's broadcast channel
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
use crate::conf_mgr::conf_center::common::{
    clear_runtime_invalid, get_runtime_invalid, parse_resource_content, store_runtime_invalid, RuntimeInvalidEnvelopes,
};
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
            fs_raw_store,
            invalid_envelopes: Arc::new(RuntimeInvalidEnvelopes::default()),
        }
    }

    /// Set shutdown signal
    pub fn with_shutdown(mut self, signal: ShutdownSignal) -> Self {
        self.shutdown_signal = Some(signal);
        self
    }

    /// Run the controller event loop
    pub async fn run(mut self) -> Result<()> {
        let kind = self.kind;

        controller_metrics().controller_started();
        tracing::info!(
            component = "fs_resource_controller",
            kind = kind,
            "Starting FileSystemResourceController"
        );

        let mut init_done = false;
        let mut init_count: usize = 0;
        let mut init_timer: Option<InitSyncTimer> = None;
        let mut worker_handle: Option<JoinHandle<()>> = None;
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
                Ok(event) => {
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
                            match parse_resource_content::<K>(&content) {
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

                                    let result = self.processor.on_init_apply(obj, existing_status_json).await;

                                    // Handle status persistence (same as runtime)
                                    match result {
                                        WorkItemResult::Processed { obj, status_changed } => {
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
                            worker_handle = Some(spawn_worker(
                                self.processor.clone(),
                                self.conf_dir.clone(),
                                self.registry.clone(),
                                kind,
                                self.shutdown_signal.clone(),
                                self.fs_raw_store.clone(),
                                self.invalid_envelopes.clone(),
                            ));

                            tracing::info!(
                                component = "fs_resource_controller",
                                kind = kind,
                                "Worker started, processing runtime events"
                            );
                        }
                        FileSystemEvent::Apply { path, content } => {
                            if !init_done {
                                // During init phase, treat as InitApply
                                match parse_resource_content::<K>(&content) {
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
                                        let result = self.processor.on_init_apply(obj, existing_status_json).await;

                                        if let WorkItemResult::Processed { obj, status_changed } = result {
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
                                match parse_resource_content::<K>(&content) {
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
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // NOTE: Unlike Kubernetes mode (which triggers a full relink on 410 Gone),
                    // FileSystem mode has no automatic recovery for lagged events. If the
                    // broadcast channel drops messages, the in-memory cache may permanently
                    // diverge from the filesystem until the controller restarts.
                    // FileSystem mode is intended for static/low-churn configs, so this risk
                    // is acceptable in practice, but monitor the warn log for anomalies.
                    tracing::warn!(
                        component = "fs_resource_controller",
                        kind = kind,
                        lagged = n,
                        "Event receiver lagged, some events may be missed — cache may diverge until restart"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => {
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
        if let Some(handle) = worker_handle {
            tracing::info!(
                component = "fs_resource_controller",
                kind = kind,
                "Waiting for worker task to finish..."
            );

            match tokio::time::timeout(Duration::from_secs(5), handle).await {
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
                    tracing::warn!(
                        component = "fs_resource_controller",
                        kind = kind,
                        "Worker task did not finish within 5 seconds"
                    );
                }
            }
        }

        controller_metrics().controller_stopped();
        tracing::info!(
            component = "fs_resource_controller",
            kind = kind,
            "FileSystemResourceController stopped"
        );

        Ok(())
    }
}

use tokio::sync::broadcast;

/// Spawn worker task for processing workqueue items
fn spawn_worker<K>(
    processor: Arc<ResourceProcessor<K>>,
    conf_dir: PathBuf,
    registry: Arc<PathRegistry>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
    fs_raw_store: Arc<LocalRawStore<K>>,
    invalid_envelopes: Arc<RuntimeInvalidEnvelopes>,
) -> JoinHandle<()>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    let workqueue = processor.workqueue();
    let status_handler = FileSystemStatusHandler::new(conf_dir.clone()).with_registry(registry.clone());

    tokio::spawn(async move {
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
                            Ok(content) => match parse_resource_content::<K>(&content) {
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
                        WorkItemResult::Processed { obj, status_changed } => {
                            if status_changed {
                                match extract_status_value(&obj) {
                                    Some(status_value) => {
                                        match status_handler
                                            .write_status_value(kind, &work_item.key, &status_value)
                                            .await
                                        {
                                            Ok(()) => {
                                                crate::conf_mgr::sync_runtime::record_status_write(kind, "filesystem");
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
                                        // A repaired statusless resource must
                                        // remove the retained ParseError sidecar.
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
mod tests {
    use super::filename_identity_mismatch;
    use super::parse_resource_content;
    use std::path::Path;

    #[test]
    fn identity_match_returns_none() {
        let path = Path::new("/conf/default/HTTPRoute_default_r1.yaml");
        assert_eq!(filename_identity_mismatch(path, "HTTPRoute", "default/r1"), None);
    }

    #[test]
    fn identity_name_change_is_detected() {
        let path = Path::new("/conf/default/HTTPRoute_default_r1.yaml");
        assert_eq!(
            filename_identity_mismatch(path, "HTTPRoute", "default/r2"),
            Some("default/r1".to_string())
        );
    }

    #[test]
    fn identity_namespace_change_is_detected() {
        let path = Path::new("/conf/HTTPRoute_default_r1.yaml");
        assert_eq!(
            filename_identity_mismatch(path, "HTTPRoute", "other/r1"),
            Some("default/r1".to_string())
        );
    }

    #[test]
    fn identity_cluster_scoped_match_returns_none() {
        let path = Path::new("/conf/GatewayClass__gc1.yaml");
        assert_eq!(filename_identity_mismatch(path, "GatewayClass", "gc1"), None);
    }

    #[test]
    fn identity_cluster_scoped_name_change_is_detected() {
        let path = Path::new("/conf/GatewayClass__gc1.yaml");
        assert_eq!(
            filename_identity_mismatch(path, "GatewayClass", "gc2"),
            Some("gc1".to_string())
        );
    }

    #[test]
    fn identity_unparseable_filename_returns_none() {
        // Filename kind does not match the controller kind → extract_key_from_path
        // returns None, so no mismatch is raised here (handled by other paths).
        let path = Path::new("/conf/random.yaml");
        assert_eq!(filename_identity_mismatch(path, "HTTPRoute", "default/r2"), None);
    }
    use edgion_resources::prelude_resources::{
        BackendTLSPolicy, BackendTLSPolicySpec, BackendTLSPolicyStatus, BackendTLSPolicyTargetRef,
        BackendTLSPolicyValidation, TLSBackendRef, TLSRoute, TLSRouteAlpha3, TLSRouteAlpha3Spec, TLSRouteRule,
        TLSRouteSpec, TLSRouteStatus,
    };
    use edgion_resources::resources::common::ParentReference;

    fn make_policy() -> BackendTLSPolicy {
        BackendTLSPolicy {
            metadata: kube::api::ObjectMeta {
                name: Some("btls".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: "".to_string(),
                    kind: "Service".to_string(),
                    name: "backend".to_string(),
                    section_name: None,
                }],
                resolved_target_class: Some(
                    edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
                ),
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: "backend.default.svc".to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: None,
                },
                options: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: false,
            },
            status: Some(BackendTLSPolicyStatus { ancestors: vec![] }),
        }
    }

    #[test]
    fn parse_backend_tls_policy_rejects_v1alpha3() {
        let yaml = serde_yaml::to_string(&make_policy()).unwrap().replacen(
            "gateway.networking.k8s.io/v1",
            "gateway.networking.k8s.io/v1alpha3",
            1,
        );

        let error = parse_resource_content::<BackendTLSPolicy>(&yaml).unwrap_err();
        assert!(error
            .to_string()
            .contains("BackendTLSPolicy apiVersion must be gateway.networking.k8s.io/v1"));
    }

    #[test]
    fn parse_backend_tls_policy_rejects_missing_api_version() {
        let yaml = serde_yaml::to_string(&make_policy())
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with("apiVersion:"))
            .collect::<Vec<_>>()
            .join("\n");

        let error = parse_resource_content::<BackendTLSPolicy>(&yaml).unwrap_err();
        assert!(error.to_string().contains("found <missing>"));
    }

    #[test]
    fn parse_backend_tls_policy_v1_keeps_canonical_type() {
        let policy = make_policy();
        let yaml = serde_yaml::to_string(&policy).unwrap();

        let parsed: BackendTLSPolicy = parse_resource_content(&yaml).unwrap();
        let serialized = serde_yaml::to_value(&parsed).unwrap();

        assert_eq!(
            serialized.get("apiVersion").and_then(|v| v.as_str()),
            Some("gateway.networking.k8s.io/v1")
        );
        assert_eq!(parsed.metadata.name.as_deref(), Some("btls"));
    }

    fn make_tls_route() -> TLSRoute {
        TLSRoute {
            metadata: kube::api::ObjectMeta {
                name: Some("tr".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some("default".to_string()),
                    name: "gw".to_string(),
                    section_name: None,
                    port: None,
                }]),
                hostnames: Some(vec!["example.com".to_string()]),
                rules: Some(vec![TLSRouteRule {
                    backend_refs: Some(vec![TLSBackendRef {
                        name: "svc".to_string(),
                        namespace: None,
                        port: Some(443),
                        weight: Some(1),
                        group: None,
                        kind: None,
                        ref_denied: None,
                    }]),
                }]),
                resolved_listeners: None,
            },
            status: Some(TLSRouteStatus { parents: vec![] }),
        }
    }

    #[test]
    fn parse_tls_route_v1alpha3_into_canonical_v1() {
        let route = make_tls_route();
        let alpha3 = TLSRouteAlpha3 {
            metadata: route.metadata.clone(),
            spec: TLSRouteAlpha3Spec {
                parent_refs: route.spec.parent_refs.clone(),
                hostnames: route.spec.hostnames.clone(),
                rules: route.spec.rules.clone(),
            },
            status: route.status.clone(),
        };
        let yaml = serde_yaml::to_string(&alpha3).unwrap();

        let parsed: TLSRoute = parse_resource_content(&yaml).unwrap();
        let serialized = serde_yaml::to_value(&parsed).unwrap();

        assert_eq!(
            serialized.get("apiVersion").and_then(|v| v.as_str()),
            Some("gateway.networking.k8s.io/v1")
        );
        assert_eq!(parsed.spec.hostnames.as_ref().unwrap()[0], "example.com");
        assert_eq!(
            parsed.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0].name,
            "svc"
        );
    }

    #[test]
    fn parse_tls_route_v1_keeps_canonical_type() {
        let route = make_tls_route();
        let yaml = serde_yaml::to_string(&route).unwrap();

        let parsed: TLSRoute = parse_resource_content(&yaml).unwrap();
        let serialized = serde_yaml::to_value(&parsed).unwrap();

        assert_eq!(
            serialized.get("apiVersion").and_then(|v| v.as_str()),
            Some("gateway.networking.k8s.io/v1")
        );
        assert_eq!(parsed.metadata.name.as_deref(), Some("tr"));
    }

    use super::{spawn_worker, FileSystemEvent, FileSystemResourceController, ResourceKey, ResourceProcessor};
    use crate::conf_mgr::conf_center::common::{clear_runtime_invalid, store_runtime_invalid, RuntimeInvalidEnvelopes};
    use crate::conf_mgr::conf_center::file_system::path_registry::PathRegistry;
    use crate::conf_mgr::conf_center::file_system::status::FileSystemStatusHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, LocalRawStore, ProcessorHandler};
    use crate::conf_mgr::sync_runtime::ShutdownHandle;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::prelude_resources::HTTPRoute;
    use edgion_resources::resources::http_route::HTTPRouteSpec;
    use kube::api::ObjectMeta;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::broadcast;
    use tokio::time::timeout;

    struct FsNoopHandler;
    #[async_trait::async_trait]
    impl ProcessorHandler<HTTPRoute> for FsNoopHandler {}

    #[tokio::test]
    async fn duplicate_init_done_keeps_one_worker() {
        struct InitCountingHandler(AtomicUsize);
        #[async_trait::async_trait]
        impl ProcessorHandler<HTTPRoute> for InitCountingHandler {
            fn on_init_done(&self, _ctx: &HandlerContext) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let handler = Arc::new(InitCountingHandler(AtomicUsize::new(0)));
        let processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
            "HTTPRoute",
            100,
            handler.clone(),
            Arc::new(ControllerRuntime::default()),
        ));
        let (event_tx, event_rx) = broadcast::channel(16);
        let shutdown = ShutdownHandle::new();
        let controller = FileSystemResourceController::new(
            "HTTPRoute",
            processor.clone(),
            tmp.path().to_path_buf(),
            Arc::new(PathRegistry::default()),
            event_rx,
        )
        .with_shutdown(shutdown.signal());
        let mut run = Box::pin(controller.run());

        event_tx.send(FileSystemEvent::Init).unwrap();
        event_tx.send(FileSystemEvent::InitDone).unwrap();
        assert!(futures::poll!(&mut run).is_pending());
        // With no resources or awaits in the init sequence, this poll drains
        // all events. Only the test, controller, and sole worker own the processor.
        assert_eq!(Arc::strong_count(&processor), 3);
        assert_eq!(handler.0.load(Ordering::Relaxed), 1);

        // Cover both a duplicate completion and a repeated empty init sequence.
        for repeat_init in [false, true] {
            if repeat_init {
                event_tx.send(FileSystemEvent::Init).unwrap();
            }
            event_tx.send(FileSystemEvent::InitDone).unwrap();
            assert!(futures::poll!(&mut run).is_pending());
            assert_eq!(
                Arc::strong_count(&processor),
                3,
                "duplicate InitDone spawned another worker"
            );
            assert_eq!(handler.0.load(Ordering::Relaxed), 1, "init callback ran more than once");
        }

        shutdown.shutdown();
        timeout(Duration::from_secs(1), &mut run).await.unwrap().unwrap();
        drop(run);
        assert_eq!(Arc::strong_count(&processor), 1, "worker was not joined on shutdown");
    }

    fn make_route(ns: &str, name: &str) -> HTTPRoute {
        HTTPRoute {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                ..Default::default()
            },
            spec: HTTPRouteSpec {
                parent_refs: None,
                hostnames: None,
                resolved_hostnames: None,
                resolved_listeners: None,
                invalid_rule_indices: None,
                rules: None,
                resolved_rules: None,
                delegation_issues: Vec::new(),
            },
            status: None,
        }
    }

    /// Build processor + worker over a temp conf dir. Returns what tests need
    /// to drive and assert; the JoinHandle keeps the worker alive.
    type FsWorkerTestHarness = (
        Arc<ResourceProcessor<HTTPRoute>>,
        Arc<PathRegistry>,
        Arc<LocalRawStore<HTTPRoute>>,
        FileSystemStatusHandler,
        Arc<RuntimeInvalidEnvelopes>,
        tokio::task::JoinHandle<()>,
    );

    fn fs_worker_test_harness(conf_dir: &std::path::Path) -> FsWorkerTestHarness {
        let processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
            "HTTPRoute",
            100,
            Arc::new(FsNoopHandler),
            Arc::new(ControllerRuntime::default()),
        ));
        let registry = Arc::new(PathRegistry::default());
        let raw_store = Arc::new(LocalRawStore::<HTTPRoute>::new());
        let status_handler = FileSystemStatusHandler::new(conf_dir.to_path_buf()).with_registry(registry.clone());
        let invalid_envelopes = Arc::new(RuntimeInvalidEnvelopes::default());
        let worker = spawn_worker(
            processor.clone(),
            conf_dir.to_path_buf(),
            registry.clone(),
            "HTTPRoute",
            None,
            raw_store.clone(),
            invalid_envelopes.clone(),
        );
        (
            processor,
            registry,
            raw_store,
            status_handler,
            invalid_envelopes,
            worker,
        )
    }

    #[tokio::test]
    async fn sole_file_delete_removes_resource_and_status() {
        let tmp = tempfile::tempdir().unwrap();
        let (processor, _registry, _raw_store, status_handler, _invalid, _worker) = fs_worker_test_harness(tmp.path());

        // Prior state: resource in cache + a persisted .status file, no spec
        // file on disk (the file was just deleted by the operator).
        let route = make_route("default", "r1");
        processor.save(route, None);
        status_handler
            .write_status_value("HTTPRoute", "default/r1", &serde_json::json!({"parents": []}))
            .await
            .unwrap();
        assert!(status_handler
            .read_status_json("HTTPRoute", "default/r1")
            .await
            .is_some());

        // Delete event arrives: key is enqueued; the worker must delete.
        processor.on_event_key("default/r1".to_string()).await;

        // Poll until the worker has processed (cache emptied), max 5s.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while processor.get("default/r1").is_some() {
            assert!(std::time::Instant::now() < deadline, "worker did not delete in time");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Status sidecar must be cleaned up too (the bonus fix from the spec).
        assert!(status_handler
            .read_status_json("HTTPRoute", "default/r1")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn never_cached_key_delete_is_skipped_without_status_write() {
        let tmp = tempfile::tempdir().unwrap();
        let (processor, _registry, _raw_store, status_handler, _invalid, _worker) = fs_worker_test_harness(tmp.path());

        // An orphan error-status file from a resource that never parsed.
        status_handler
            .write_status_value(
                "HTTPRoute",
                "default/ghost",
                &serde_json::json!({"error": "ParseError"}),
            )
            .await
            .unwrap();

        // Delete event for a key in neither cache nor store: worker must skip.
        processor.on_event_key("default/ghost".to_string()).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        assert!(processor.get("default/ghost").is_none());
        // Documented residual (spec: "Bonus fix" section): the orphan status
        // file is NOT cleaned by the Skipped arm — same as before this change.
        assert!(status_handler
            .read_status_json("HTTPRoute", "default/ghost")
            .await
            .is_some());
    }

    #[tokio::test]
    async fn delete_then_recreate_before_dequeue_reapplies() {
        let tmp = tempfile::tempdir().unwrap();
        let (processor, _registry, _raw_store, _status_handler, _invalid, _worker) = fs_worker_test_harness(tmp.path());

        // v1 is cached; the file is deleted (delete event pending), but a new
        // file for the same key reappears before the worker dequeues — the
        // level-triggered outcome must be re-apply, not delete.
        let v1 = make_route("default", "r1");
        processor.save(v1, None);

        // Recreate on disk at the canonical fallback path (registry has no
        // record, so the worker falls back to build_resource_path_from_key).
        let mut v2 = make_route("default", "r1");
        v2.spec.hostnames = Some(vec!["v2.example.com".to_string()]);
        let path = tmp.path().join("default").join("HTTPRoute_default_r1.yaml");
        tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        tokio::fs::write(&path, serde_yaml::to_string(&v2).unwrap())
            .await
            .unwrap();

        processor.on_event_key("default/r1".to_string()).await;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(cur) = processor.get("default/r1") {
                if cur.spec.hostnames.as_deref() == Some(&["v2.example.com".to_string()]) {
                    break; // re-applied with v2 content
                }
            } else {
                panic!("resource was deleted; expected level-triggered re-apply");
            }
            assert!(std::time::Instant::now() < deadline, "worker did not re-apply in time");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn valid_to_malformed_to_repair_revokes_then_recovers_via_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let (processor, _registry, raw_store, status_handler, invalid, _worker) = fs_worker_test_harness(tmp.path());
        let key = "default/r1";
        let path = tmp.path().join("default").join("HTTPRoute_default_r1.yaml");
        tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();

        let v1 = make_route("default", "r1");
        tokio::fs::write(&path, serde_yaml::to_string(&v1).unwrap())
            .await
            .unwrap();
        raw_store.upsert(key.to_string(), Arc::new(v1.clone()));
        processor.save(v1, None);

        raw_store.remove(key);
        store_runtime_invalid(
            &invalid,
            key.to_string(),
            "ParseError",
            "invalid listener structure".to_string(),
        );
        processor.on_event_key(key.to_string()).await;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while processor.get(key).is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not revoke malformed object"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let error_status = status_handler.read_status_json("HTTPRoute", key).await.unwrap();
        assert!(error_status.contains("ParseError"));

        let mut v2 = make_route("default", "r1");
        v2.spec.hostnames = Some(vec!["repaired.example.com".to_string()]);
        tokio::fs::write(&path, serde_yaml::to_string(&v2).unwrap())
            .await
            .unwrap();
        clear_runtime_invalid(&invalid, key);
        raw_store.upsert(key.to_string(), Arc::new(v2));
        processor.on_event_key(key.to_string()).await;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if processor
                .get(key)
                .is_some_and(|route| route.spec.hostnames.as_deref() == Some(&["repaired.example.com".to_string()]))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not apply repaired object"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let repaired_status = status_handler.read_status_json("HTTPRoute", key).await;
        assert!(repaired_status.is_none_or(|status| !status.contains("ParseError")));
    }

    #[tokio::test]
    async fn valid_to_identity_mismatch_to_repair_revokes_then_recovers_via_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
            "HTTPRoute",
            100,
            Arc::new(FsNoopHandler),
            Arc::new(ControllerRuntime::default()),
        ));
        let registry = Arc::new(PathRegistry::default());
        let key = "default/r1";
        let path = tmp.path().join("default").join("HTTPRoute_default_r1.yaml");
        tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        registry.record(ResourceKey::new("HTTPRoute", key), path.clone());
        let status_handler = FileSystemStatusHandler::new(tmp.path().to_path_buf()).with_registry(registry.clone());
        let (event_tx, event_rx) = tokio::sync::broadcast::channel(16);
        let shutdown = crate::conf_mgr::sync_runtime::ShutdownHandle::new();
        let controller = FileSystemResourceController::new(
            "HTTPRoute",
            processor.clone(),
            tmp.path().to_path_buf(),
            registry,
            event_rx,
        )
        .with_shutdown(shutdown.signal());
        let controller_task = tokio::spawn(controller.run());

        let v1 = make_route("default", "r1");
        tokio::fs::write(&path, serde_yaml::to_string(&v1).unwrap())
            .await
            .unwrap();
        event_tx.send(FileSystemEvent::Init).unwrap();
        event_tx
            .send(FileSystemEvent::InitApply {
                path: path.clone(),
                content: serde_yaml::to_string(&v1).unwrap(),
            })
            .unwrap();
        event_tx.send(FileSystemEvent::InitDone).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while processor.get(key).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "controller did not apply initial object"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let mismatched = make_route("default", "other");
        let mismatched_content = serde_yaml::to_string(&mismatched).unwrap();
        tokio::fs::write(&path, &mismatched_content).await.unwrap();
        // The watcher payload still describes the authoritative key, but the
        // file is replaced before the sole worker re-reads it. This exercises
        // the production dequeue-time identity check rather than the earlier
        // event-parser check.
        let mut queued = make_route("default", "r1");
        queued.spec.hostnames = Some(vec!["queued.example.com".to_string()]);
        event_tx
            .send(FileSystemEvent::Apply {
                path: path.clone(),
                content: serde_yaml::to_string(&queued).unwrap(),
            })
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while processor.get(key).is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not revoke identity-mismatched object"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let error_status = status_handler.read_status_json("HTTPRoute", key).await.unwrap();
        assert!(error_status.contains("IdentityMismatch"));

        let mut repaired = make_route("default", "r1");
        repaired.spec.hostnames = Some(vec!["repaired.example.com".to_string()]);
        let repaired_content = serde_yaml::to_string(&repaired).unwrap();
        tokio::fs::write(&path, &repaired_content).await.unwrap();
        event_tx
            .send(FileSystemEvent::Apply {
                path: path.clone(),
                content: repaired_content,
            })
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if processor
                .get(key)
                .is_some_and(|route| route.spec.hostnames.as_deref() == Some(&["repaired.example.com".to_string()]))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not apply repaired identity"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let repaired_status = status_handler.read_status_json("HTTPRoute", key).await;
        assert!(repaired_status.is_none_or(|status| !status.contains("IdentityMismatch")));
        shutdown.shutdown();
        assert!(controller_task.await.unwrap().is_ok());
    }
}
