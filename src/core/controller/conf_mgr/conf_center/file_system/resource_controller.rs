//! FileSystemResourceController - Per-resource controller for FileSystem mode
//!
//! This controller receives events from FileSystemWatcher's broadcast channel
//! and processes them using the same logic as Kubernetes mode.
//!
//! The event handling logic mirrors K8s mode's ResourceController:
//! - Init -> processor.on_init()
//! - InitApply { path, content } -> parse + processor.on_init_apply(obj)
//! - InitDone -> processor.on_init_done()
//! - Apply { path, content } -> parse + processor.on_apply(&obj)
//! - Delete(info) -> processor.on_delete(&obj)

use super::event::FileSystemEvent;
use super::file_watcher::KindEventReceiver;
use super::path_registry::{PathRegistry, ResourceKey};
use super::status::FileSystemStatusHandler;
use super::storage::build_resource_path_from_key;
use crate::core::controller::conf_mgr::conf_center::common::parse_resource_content;
use crate::core::controller::conf_mgr::sync_runtime::metrics::{controller_metrics, InitSyncTimer};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    extract_status_value, make_resource_key, LocalRawStore, ResourceProcessor, WorkItemResult,
};
use crate::core::controller::conf_mgr::sync_runtime::ShutdownSignal;
use crate::types::ResourceMeta;
use anyhow::Result;
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
        let raw_store: std::sync::Arc<
            dyn crate::core::controller::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore<K>,
        > = fs_raw_store.clone();
        processor.set_raw_store(raw_store.clone());
        crate::core::controller::observe::status_metrics::register_status_renderer::<K>(kind, raw_store);
        Self {
            kind,
            processor,
            conf_dir,
            registry,
            event_rx,
            shutdown_signal: None,
            fs_raw_store,
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
                                // Already initialized, this might be a re-init
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
                                                                crate::core::controller::conf_mgr::sync_runtime::record_status_write(kind, "filesystem");
                                                                tracing::info!(
                                                                    component = "fs_resource_controller",
                                                                    kind = kind,
                                                                    key = %key,
                                                                    "Persisted status to filesystem during init"
                                                                );
                                                            }
                                                            Err(e) => {
                                                                crate::core::controller::conf_mgr::sync_runtime::record_status_write_error(kind, "filesystem");
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
                            init_done = true;

                            // Spawn worker for runtime phase
                            worker_handle = Some(spawn_worker(
                                self.processor.clone(),
                                self.conf_dir.clone(),
                                self.registry.clone(),
                                kind,
                                self.shutdown_signal.clone(),
                                self.fs_raw_store.clone(),
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
                                                            crate::core::controller::conf_mgr::sync_runtime::record_status_write(kind, "filesystem");
                                                            tracing::info!(
                                                                component = "fs_resource_controller",
                                                                kind = kind,
                                                                key = %key,
                                                                "Persisted status to filesystem during apply-as-init"
                                                            );
                                                        }
                                                        Err(e) => {
                                                            crate::core::controller::conf_mgr::sync_runtime::record_status_write_error(kind, "filesystem");
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
                                        tracing::info!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            key = %key,
                                            path = %path.display(),
                                            "Received runtime Apply event, enqueuing"
                                        );
                                        self.fs_raw_store.upsert(key, Arc::new(obj.clone()));
                                        self.processor.on_apply(&obj).await;
                                    }
                                    Err(e) => {
                                        // File content is invalid — evict any stale entry from
                                        // LocalRawStore so cross-resource lookups don't return
                                        // a previously-valid but now-outdated version.
                                        if let Some(key) = extract_key_from_path(&path, kind) {
                                            self.fs_raw_store.remove(&key);
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
                                    // Remove from raw store immediately on delete.
                                    self.fs_raw_store.remove(&key);
                                    // For delete, we need to get the cached object
                                    // The worker will handle the actual deletion
                                    if let Some(obj) = self.processor.get(&key) {
                                        self.processor.on_delete(&obj).await;
                                    } else {
                                        tracing::trace!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            key = %key,
                                            "Delete event for non-cached resource, ignoring"
                                        );
                                    }
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
                                    // Keep raw store in sync with the latest file version.
                                    // The main loop upserts on Apply events, but the worker
                                    // re-reads the file (possibly a newer version), so we
                                    // update here to stay consistent.
                                    fs_raw_store.upsert(work_item.key.clone(), Arc::new(obj.clone()));
                                    (Some(obj), None)
                                }
                                Err(e) => {
                                    let error_msg = e.to_string();
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

                    // Read existing status from .status file for comparison
                    let existing_status_json = status_handler.read_status_json(kind, &work_item.key).await;

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
                                                crate::core::controller::conf_mgr::sync_runtime::record_status_write(
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
                                                crate::core::controller::conf_mgr::sync_runtime::record_status_write_error(kind, "filesystem");
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
                                            "Status changed but failed to extract status value for persistence"
                                        );
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
    use super::parse_resource_content;
    use crate::types::prelude_resources::{
        BackendTLSPolicy, BackendTLSPolicyAlpha3, BackendTLSPolicyAlpha3Spec, BackendTLSPolicySpec,
        BackendTLSPolicyStatus, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation, TLSBackendRef, TLSRoute,
        TLSRouteAlpha3, TLSRouteAlpha3Spec, TLSRouteRule, TLSRouteSpec, TLSRouteStatus,
    };
    use crate::types::resources::common::ParentReference;

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
    fn parse_backend_tls_policy_v1alpha3_into_canonical_v1() {
        let policy = make_policy();
        let alpha3 = BackendTLSPolicyAlpha3 {
            metadata: policy.metadata.clone(),
            spec: BackendTLSPolicyAlpha3Spec {
                target_refs: policy.spec.target_refs.clone(),
                validation: policy.spec.validation.clone(),
                options: policy.spec.options.clone(),
                resolved_ca_certificates: policy.spec.resolved_ca_certificates.clone(),
                resolved_client_certificate: policy.spec.resolved_client_certificate.clone(),
                use_system_ca: policy.spec.use_system_ca,
            },
            status: policy.status.clone(),
        };
        let yaml = serde_yaml::to_string(&alpha3).unwrap();

        let parsed: BackendTLSPolicy = parse_resource_content(&yaml).unwrap();
        let serialized = serde_yaml::to_value(&parsed).unwrap();

        assert_eq!(
            serialized.get("apiVersion").and_then(|v| v.as_str()),
            Some("gateway.networking.k8s.io/v1")
        );
        assert_eq!(parsed.spec.validation.hostname, "backend.default.svc");
        assert_eq!(parsed.spec.target_refs[0].name, "backend");
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
                        extension_info: Default::default(),
                        plugin_runtime: Default::default(),
                        ref_denied: None,
                    }]),
                    backend_finder: Default::default(),
                    plugin_runtime: Default::default(),
                    proxy_protocol_version: None,
                    stream_plugin_store_key: None,
                    max_connect_retries: 1,
                    tcp_keepalive: None,
                }]),
                resolved_listeners: None,
                effective_hostnames: None,
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
}
