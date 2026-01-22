//! FileSystem Sync Controller
//!
//! Unified controller for FileSystem mode that:
//! - Init phase: scans directory and processes resources directly (no workqueue)
//! - Runtime phase: monitors file changes via workqueue
//!
//! This mirrors the K8s mode's flow:
//! - K8s: InitApply → direct process, Apply/Delete → workqueue
//! - FileSystem: scan → direct process, file changes → workqueue

use super::tracker::FileResourceTracker;
use crate::core::conf_mgr::conf_center::sync_runtime::{
    process_resource, process_resource_delete, BackendTlsPolicyProcessor, EdgionGatewayConfigProcessor,
    EdgionPluginsProcessor, EdgionStreamPluginsProcessor, EdgionTlsProcessor, EndpointSliceProcessor,
    EndpointsProcessor, GatewayClassProcessor, GatewayProcessor, GrpcRouteProcessor, HttpRouteProcessor,
    LinkSysProcessor, PluginMetadataProcessor, ProcessConfig, ProcessContext, ReferenceGrantProcessor,
    RequeueRegistry, ResourceProcessor, SecretProcessor, ServiceProcessor, ShutdownSignal, TcpRouteProcessor,
    TlsRouteProcessor, UdpRouteProcessor, Workqueue,
};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::types::prelude_resources::*;
use crate::types::resource::ALL_RESOURCE_INFOS;
use crate::types::ResourceKind;
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::de::DeserializeOwned;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// FileSystem Sync Controller
///
/// Manages resource synchronization from local YAML files to ConfigServer.
/// Uses the same ResourceProcessor-based flow as K8s mode.
pub struct FileSystemSyncController {
    conf_dir: PathBuf,
    config_server: Arc<ConfigServer>,
    tracker: Arc<RwLock<FileResourceTracker>>,
    requeue_registry: Arc<RequeueRegistry>,
    /// Workqueue per resource kind
    workqueues: HashMap<&'static str, Arc<Workqueue>>,
    process_config: ProcessConfig,
}

impl FileSystemSyncController {
    /// Create a new FileSystemSyncController
    pub fn new(conf_dir: PathBuf, config_server: Arc<ConfigServer>) -> Self {
        let requeue_registry = Arc::new(RequeueRegistry::new());

        // Create workqueues for each resource kind
        let mut workqueues = HashMap::new();
        for info in ALL_RESOURCE_INFOS {
            // Use ResourceKind::as_str() for stable static string
            let kind_name = info.kind.as_str();
            let queue = Arc::new(Workqueue::with_defaults(kind_name));
            requeue_registry.register(kind_name, queue.clone());
            workqueues.insert(kind_name, queue);
        }

        Self {
            conf_dir,
            config_server,
            tracker: Arc::new(RwLock::new(FileResourceTracker::new())),
            requeue_registry,
            workqueues,
            process_config: ProcessConfig::default(),
        }
    }

    /// Create a controller for reload (no runtime phase)
    pub fn new_for_reload(conf_dir: PathBuf, config_server: Arc<ConfigServer>) -> Self {
        Self::new(conf_dir, config_server)
    }

    /// Run the complete lifecycle: init phase + runtime phase
    pub async fn run(&self, mut shutdown_signal: ShutdownSignal) -> Result<()> {
        tracing::info!(
            component = "fs_sync_controller",
            conf_dir = %self.conf_dir.display(),
            "Starting FileSystem sync controller"
        );

        // Phase 1: Init - scan and load all resources
        self.init_phase().await?;

        // Mark all caches as ready
        for info in ALL_RESOURCE_INFOS {
            self.config_server.set_cache_ready_by_kind(info.kind.as_str());
        }
        tracing::info!(
            component = "fs_sync_controller",
            "Init phase complete, all caches marked ready"
        );

        // Phase 2: Runtime - watch for file changes
        self.runtime_phase(&mut shutdown_signal).await?;

        tracing::info!(
            component = "fs_sync_controller",
            "FileSystem sync controller stopped"
        );

        Ok(())
    }

    /// Init phase: scan directory and process all resources directly
    ///
    /// This is also used by reload API.
    pub async fn init_phase(&self) -> Result<()> {
        tracing::info!(
            component = "fs_sync_controller",
            conf_dir = %self.conf_dir.display(),
            "Starting init phase: scanning directory"
        );

        let mut processed = 0;
        let mut errors = 0;

        // Scan directory for YAML files
        let entries = std::fs::read_dir(&self.conf_dir)
            .with_context(|| format!("Failed to read directory: {}", self.conf_dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Skip non-YAML files
            if !is_yaml_file(&path) {
                continue;
            }

            // Process the file
            match self.process_file_init(&path).await {
                Ok(true) => processed += 1,
                Ok(false) => {} // Skipped (filtered)
                Err(e) => {
                    errors += 1;
                    tracing::error!(
                        component = "fs_sync_controller",
                        path = %path.display(),
                        error = %e,
                        "Failed to process file during init"
                    );
                }
            }
        }

        tracing::info!(
            component = "fs_sync_controller",
            processed = processed,
            errors = errors,
            "Init phase complete"
        );

        Ok(())
    }

    /// Process a single file during init phase (direct processing, no workqueue)
    async fn process_file_init(&self, path: &Path) -> Result<bool> {
        let content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read file: {}", path.display()))?;

        let hash = compute_hash(&content);

        // Determine resource kind from content
        let kind = ResourceKind::from_content(&content);
        let Some(kind) = kind else {
            tracing::debug!(
                component = "fs_sync_controller",
                path = %path.display(),
                "Skipping file: could not determine resource kind"
            );
            return Ok(false);
        };

        // Create ProcessContext
        let ctx = ProcessContext::new(
            &self.config_server,
            self.process_config.metadata_filter.as_ref(),
            None, // No namespace filter for FileSystem mode
            &self.requeue_registry,
        );

        // Process based on kind
        let result = self.process_by_kind(&content, kind, &ctx, true)?;

        if result {
            // Track the file-resource mapping
            // We need to extract the key from the parsed resource
            if let Some(key) = extract_resource_key(&content, kind) {
                self.tracker
                    .write()
                    .unwrap()
                    .track(path.to_path_buf(), kind.as_str(), &key, hash);
            }
        }

        Ok(result)
    }

    /// Runtime phase: watch for file changes
    async fn runtime_phase(&self, shutdown_signal: &mut ShutdownSignal) -> Result<()> {
        tracing::info!(
            component = "fs_sync_controller",
            conf_dir = %self.conf_dir.display(),
            "Starting runtime phase: watching for file changes"
        );

        // Create channel for file events
        let (tx, mut rx) = mpsc::channel::<Event>(100);

        // Create file watcher
        // Note: The watcher must be kept alive for the duration of the runtime phase.
        // Dropping it would stop file monitoring.
        let conf_dir = self.conf_dir.clone();
        let watcher = tokio::task::spawn_blocking(move || -> Result<RecommendedWatcher> {
            let tx_clone = tx.clone();
            let mut watcher = RecommendedWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        match event.kind {
                            EventKind::Access(_) => {} // Ignore read-only access
                            _ => {
                                let _ = tx_clone.blocking_send(event);
                            }
                        }
                    }
                },
                Config::default().with_poll_interval(Duration::from_secs(2)),
            )
            .context("Failed to create file watcher")?;

            watcher
                .watch(&conf_dir, RecursiveMode::NonRecursive)
                .context("Failed to start watching directory")?;

            Ok(watcher)
        })
        .await??;

        // Spawn workers for each resource kind
        let worker_handles = self.spawn_workers(shutdown_signal.clone());

        // Process events with debouncing
        let mut pending_paths: HashSet<PathBuf> = HashSet::new();
        let debounce_duration = Duration::from_secs(1);
        let mut debounce_timer = tokio::time::interval(debounce_duration);

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    for path in event.paths {
                        if is_yaml_file(&path) {
                            pending_paths.insert(path);
                        }
                    }
                }
                _ = debounce_timer.tick() => {
                    if !pending_paths.is_empty() {
                        for path in pending_paths.drain() {
                            if let Err(e) = self.handle_file_event(&path).await {
                                tracing::error!(
                                    component = "fs_sync_controller",
                                    path = %path.display(),
                                    error = %e,
                                    "Failed to handle file event"
                                );
                            }
                        }
                    }
                }
                _ = shutdown_signal.wait() => {
                    tracing::info!(
                        component = "fs_sync_controller",
                        "Received shutdown signal, stopping runtime phase"
                    );
                    break;
                }
            }
        }

        // Wait for workers to finish
        for handle in worker_handles {
            handle.abort();
        }

        // Explicitly drop watcher to stop file monitoring
        drop(watcher);

        Ok(())
    }

    /// Handle a file event (create/modify/delete)
    async fn handle_file_event(&self, path: &Path) -> Result<()> {
        if path.exists() {
            // File exists: Created or Modified
            let content = tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("Failed to read file: {}", path.display()))?;

            let new_hash = compute_hash(&content);
            let old_hash = self.tracker.read().unwrap().get_hash(path);

            // Skip if content hasn't changed
            if old_hash == Some(new_hash) {
                tracing::trace!(
                    component = "fs_sync_controller",
                    path = %path.display(),
                    "File content unchanged, skipping"
                );
                return Ok(());
            }

            // Determine resource kind
            let kind = ResourceKind::from_content(&content);
            let Some(kind) = kind else {
                tracing::debug!(
                    component = "fs_sync_controller",
                    path = %path.display(),
                    "Skipping file: could not determine resource kind"
                );
                return Ok(());
            };

            // Extract resource key
            let Some(key) = extract_resource_key(&content, kind) else {
                tracing::warn!(
                    component = "fs_sync_controller",
                    path = %path.display(),
                    "Could not extract resource key"
                );
                return Ok(());
            };

            let kind_str = kind.as_str();

            // Update tracker
            self.tracker
                .write()
                .unwrap()
                .track(path.to_path_buf(), kind_str, &key, new_hash);

            // Enqueue to workqueue
            if let Some(queue) = self.workqueues.get(kind_str) {
                queue.enqueue(key).await;
                tracing::debug!(
                    component = "fs_sync_controller",
                    path = %path.display(),
                    kind = %kind_str,
                    "Enqueued file change"
                );
            }
        } else {
            // File deleted
            let info = self.tracker.write().unwrap().untrack(path);
            if let Some((kind, key)) = info {
                // Enqueue for deletion processing
                if let Some(queue) = self.workqueues.get(kind.as_str()) {
                    queue.enqueue(key.clone()).await;
                    tracing::debug!(
                        component = "fs_sync_controller",
                        path = %path.display(),
                        kind = %kind,
                        key = %key,
                        "Enqueued file deletion"
                    );
                }
            }
        }

        Ok(())
    }

    /// Spawn worker tasks for each resource kind
    fn spawn_workers(&self, shutdown_signal: ShutdownSignal) -> Vec<JoinHandle<()>> {
        let mut handles = Vec::new();

        // Spawn a worker for each resource kind that has a workqueue
        for (kind, queue) in &self.workqueues {
            let worker = self.spawn_worker_for_kind(kind, queue.clone(), shutdown_signal.clone());
            if let Some(handle) = worker {
                handles.push(handle);
            }
        }

        handles
    }

    /// Spawn a worker for a specific resource kind
    fn spawn_worker_for_kind(
        &self,
        kind: &'static str,
        queue: Arc<Workqueue>,
        mut shutdown_signal: ShutdownSignal,
    ) -> Option<JoinHandle<()>> {
        let config_server = self.config_server.clone();
        let tracker = self.tracker.clone();
        let requeue_registry = self.requeue_registry.clone();
        let process_config = self.process_config.clone();

        Some(tokio::spawn(async move {
            loop {
                let item = tokio::select! {
                    item = queue.dequeue() => {
                        match item {
                            Some(item) => item,
                            None => break, // Channel closed
                        }
                    }
                    _ = shutdown_signal.wait() => break,
                };

                let key = item.key;

                // Process the work item
                process_work_item(
                    kind,
                    &key,
                    &config_server,
                    &tracker,
                    &requeue_registry,
                    &process_config,
                );

                queue.done(&key);
            }
        }))
    }

    /// Process a resource by its kind (used during init phase)
    fn process_by_kind(
        &self,
        content: &str,
        kind: ResourceKind,
        ctx: &ProcessContext,
        is_init: bool,
    ) -> Result<bool> {
        match kind {
            ResourceKind::GatewayClass => {
                process_typed::<GatewayClass, _>(content, &GatewayClassProcessor::new(), ctx, is_init, "GatewayClass")
            }
            ResourceKind::Gateway => {
                process_typed::<Gateway, _>(content, &GatewayProcessor::new(None), ctx, is_init, "Gateway")
            }
            ResourceKind::HTTPRoute => {
                process_typed::<HTTPRoute, _>(content, &HttpRouteProcessor::new(), ctx, is_init, "HTTPRoute")
            }
            ResourceKind::GRPCRoute => {
                process_typed::<GRPCRoute, _>(content, &GrpcRouteProcessor::new(), ctx, is_init, "GRPCRoute")
            }
            ResourceKind::TCPRoute => {
                process_typed::<TCPRoute, _>(content, &TcpRouteProcessor::new(), ctx, is_init, "TCPRoute")
            }
            ResourceKind::UDPRoute => {
                process_typed::<UDPRoute, _>(content, &UdpRouteProcessor::new(), ctx, is_init, "UDPRoute")
            }
            ResourceKind::TLSRoute => {
                process_typed::<TLSRoute, _>(content, &TlsRouteProcessor::new(), ctx, is_init, "TLSRoute")
            }
            ResourceKind::Service => {
                process_typed::<Service, _>(content, &ServiceProcessor::new(), ctx, is_init, "Service")
            }
            ResourceKind::Endpoint => {
                process_typed::<Endpoints, _>(content, &EndpointsProcessor::new(), ctx, is_init, "Endpoint")
            }
            ResourceKind::EndpointSlice => {
                process_typed::<EndpointSlice, _>(content, &EndpointSliceProcessor::new(), ctx, is_init, "EndpointSlice")
            }
            ResourceKind::Secret => {
                process_typed::<Secret, _>(content, &SecretProcessor::new(), ctx, is_init, "Secret")
            }
            ResourceKind::ReferenceGrant => {
                process_typed::<ReferenceGrant, _>(content, &ReferenceGrantProcessor::new(), ctx, is_init, "ReferenceGrant")
            }
            ResourceKind::BackendTLSPolicy => {
                process_typed::<BackendTLSPolicy, _>(content, &BackendTlsPolicyProcessor::new(), ctx, is_init, "BackendTLSPolicy")
            }
            ResourceKind::EdgionGatewayConfig => {
                process_typed::<EdgionGatewayConfig, _>(
                    content,
                    &EdgionGatewayConfigProcessor::new(),
                    ctx,
                    is_init,
                    "EdgionGatewayConfig",
                )
            }
            ResourceKind::EdgionTls => {
                process_typed::<EdgionTls, _>(content, &EdgionTlsProcessor::new(), ctx, is_init, "EdgionTls")
            }
            ResourceKind::EdgionPlugins => {
                process_typed::<EdgionPlugins, _>(content, &EdgionPluginsProcessor::new(), ctx, is_init, "EdgionPlugins")
            }
            ResourceKind::EdgionStreamPlugins => {
                process_typed::<EdgionStreamPlugins, _>(
                    content,
                    &EdgionStreamPluginsProcessor::new(),
                    ctx,
                    is_init,
                    "EdgionStreamPlugins",
                )
            }
            ResourceKind::PluginMetaData => {
                process_typed::<PluginMetaData, _>(content, &PluginMetadataProcessor::new(), ctx, is_init, "PluginMetaData")
            }
            ResourceKind::LinkSys => {
                process_typed::<LinkSys, _>(content, &LinkSysProcessor::new(), ctx, is_init, "LinkSys")
            }
            ResourceKind::Unspecified => {
                tracing::trace!(component = "fs_sync_controller", "Skipping Unspecified resource kind");
                Ok(false)
            }
        }
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Check if a path is a YAML file
fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext == "yaml" || ext == "yml")
        .unwrap_or(false)
}

/// Compute content hash
fn compute_hash(content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Extract resource key from YAML content
fn extract_resource_key(content: &str, _kind: ResourceKind) -> Option<String> {
    // Parse just the metadata to get namespace/name
    #[derive(serde::Deserialize)]
    struct MinimalMeta {
        metadata: MetaFields,
    }
    #[derive(serde::Deserialize)]
    struct MetaFields {
        namespace: Option<String>,
        name: Option<String>,
    }

    let meta: MinimalMeta = serde_yaml::from_str(content).ok()?;
    let name = meta.metadata.name?;

    Some(match meta.metadata.namespace {
        Some(ns) => format!("{}/{}", ns, name),
        None => name,
    })
}

/// Process a typed resource from YAML content
fn process_typed<K, P>(
    content: &str,
    processor: &P,
    ctx: &ProcessContext,
    is_init: bool,
    kind: &'static str,
) -> Result<bool>
where
    K: Resource + Clone + Send + Sync + DeserializeOwned + 'static,
    P: ResourceProcessor<K>,
{
    let obj: K = serde_yaml::from_str(content).context("Failed to parse YAML")?;
    Ok(process_resource(obj, processor, ctx, is_init, kind))
}

/// Process a work item from workqueue (runtime phase)
///
/// Similar to K8s mode's process_work_item, but uses tracker instead of Store
fn process_work_item(
    kind: &'static str,
    key: &str,
    config_server: &ConfigServer,
    tracker: &RwLock<FileResourceTracker>,
    requeue_registry: &RequeueRegistry,
    process_config: &ProcessConfig,
) {
    let ctx = ProcessContext::new(
        config_server,
        process_config.metadata_filter.as_ref(),
        None,
        requeue_registry,
    );

    // Check if resource is still tracked (file exists)
    let path = {
        let tracker_guard = tracker.read().unwrap();
        tracker_guard.get_path_by_key(kind, key).cloned()
    };

    match kind {
        "GatewayClass" => process_work_item_typed::<GatewayClass, _>(
            key,
            path.as_deref(),
            &GatewayClassProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "Gateway" => process_work_item_typed::<Gateway, _>(
            key,
            path.as_deref(),
            &GatewayProcessor::new(None),
            &ctx,
            config_server,
            kind,
        ),
        "HTTPRoute" => process_work_item_typed::<HTTPRoute, _>(
            key,
            path.as_deref(),
            &HttpRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "GRPCRoute" => process_work_item_typed::<GRPCRoute, _>(
            key,
            path.as_deref(),
            &GrpcRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "TCPRoute" => process_work_item_typed::<TCPRoute, _>(
            key,
            path.as_deref(),
            &TcpRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "UDPRoute" => process_work_item_typed::<UDPRoute, _>(
            key,
            path.as_deref(),
            &UdpRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "TLSRoute" => process_work_item_typed::<TLSRoute, _>(
            key,
            path.as_deref(),
            &TlsRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "Service" => process_work_item_typed::<Service, _>(
            key,
            path.as_deref(),
            &ServiceProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "Endpoint" => process_work_item_typed::<Endpoints, _>(
            key,
            path.as_deref(),
            &EndpointsProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EndpointSlice" => process_work_item_typed::<EndpointSlice, _>(
            key,
            path.as_deref(),
            &EndpointSliceProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "Secret" => process_work_item_typed::<Secret, _>(
            key,
            path.as_deref(),
            &SecretProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "ReferenceGrant" => process_work_item_typed::<ReferenceGrant, _>(
            key,
            path.as_deref(),
            &ReferenceGrantProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "BackendTLSPolicy" => process_work_item_typed::<BackendTLSPolicy, _>(
            key,
            path.as_deref(),
            &BackendTlsPolicyProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EdgionGatewayConfig" => process_work_item_typed::<EdgionGatewayConfig, _>(
            key,
            path.as_deref(),
            &EdgionGatewayConfigProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EdgionTls" => process_work_item_typed::<EdgionTls, _>(
            key,
            path.as_deref(),
            &EdgionTlsProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EdgionPlugins" => process_work_item_typed::<EdgionPlugins, _>(
            key,
            path.as_deref(),
            &EdgionPluginsProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EdgionStreamPlugins" => process_work_item_typed::<EdgionStreamPlugins, _>(
            key,
            path.as_deref(),
            &EdgionStreamPluginsProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "PluginMetaData" => process_work_item_typed::<PluginMetaData, _>(
            key,
            path.as_deref(),
            &PluginMetadataProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "LinkSys" => process_work_item_typed::<LinkSys, _>(
            key,
            path.as_deref(),
            &LinkSysProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        _ => {
            tracing::warn!(
                component = "fs_sync_controller",
                kind = kind,
                key = key,
                "Unknown resource kind, skipping"
            );
        }
    }
}

/// Process a typed work item
fn process_work_item_typed<K, P>(
    key: &str,
    path: Option<&Path>,
    processor: &P,
    ctx: &ProcessContext,
    config_server: &ConfigServer,
    kind: &'static str,
) where
    K: Resource + Clone + Send + Sync + DeserializeOwned + 'static,
    P: ResourceProcessor<K>,
{
    let cache_obj = processor.get(config_server, key);

    match (path, cache_obj) {
        (Some(path), _) => {
            // File exists (tracked) → read and process
            match std::fs::read_to_string(path) {
                Ok(content) => match serde_yaml::from_str::<K>(&content) {
                    Ok(obj) => {
                        process_resource(obj, processor, ctx, false, kind);
                    }
                    Err(e) => {
                        tracing::error!(
                            component = "fs_sync_controller",
                            kind = kind,
                            key = key,
                            path = %path.display(),
                            error = %e,
                            "Failed to parse file"
                        );
                    }
                },
                Err(e) => {
                    tracing::error!(
                        component = "fs_sync_controller",
                        kind = kind,
                        key = key,
                        path = %path.display(),
                        error = %e,
                        "Failed to read file"
                    );
                }
            }
        }
        (None, Some(cached)) => {
            // File deleted but cache has it → delete from cache
            process_resource_delete(cached, processor, ctx, kind);
        }
        (None, None) => {
            // Neither file nor cache → already processed
            tracing::trace!(
                component = "fs_sync_controller",
                kind = kind,
                key = key,
                "Resource not found in tracker or cache, skipping"
            );
        }
    }
}
