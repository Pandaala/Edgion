//! FileSystem Sync Controller
//!
//! Unified controller for FileSystem mode that:
//! - Init phase: scans directory and processes resources directly (no workqueue)
//! - Runtime phase: monitors file changes via workqueue
//!
//! This mirrors the K8s mode's flow:
//! - K8s: InitApply → direct process, Apply/Delete → workqueue
//! - FileSystem: scan → direct process, file changes → workqueue
//!
//! File naming convention:
//! - With namespace: `{Kind}_{namespace}_{name}.yaml`
//! - Cluster-scoped: `{Kind}__{name}.yaml` (double underscore)

use crate::core::conf_mgr::conf_center::sync_runtime::{
    process_resource, process_resource_delete, BackendTlsPolicyProcessor, EdgionGatewayConfigProcessor,
    EdgionPluginsProcessor, EdgionStreamPluginsProcessor, EdgionTlsProcessor, EndpointSliceProcessor,
    EndpointsProcessor, GatewayClassProcessor, GatewayProcessor, GrpcRouteProcessor, HttpRouteProcessor,
    LinkSysProcessor, PluginMetadataProcessor, ProcessConfig, ProcessContext, ReferenceGrantProcessor,
    RequeueRegistry, ResourceProcessor, SecretProcessor, SecretRefManager, ServiceProcessor, ShutdownSignal,
    TcpRouteProcessor, TlsRouteProcessor, UdpRouteProcessor, Workqueue,
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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// FileSystem Sync Controller
///
/// Manages resource synchronization from local YAML files to ConfigServer.
/// Uses the same ResourceProcessor-based flow as K8s mode.
///
/// Key simplification: Uses file naming convention `Kind_namespace_name.yaml`
/// to determine resource identity, eliminating the need for tracking state.
pub struct FileSystemSyncController {
    conf_dir: PathBuf,
    config_server: Arc<ConfigServer>,
    requeue_registry: Arc<RequeueRegistry>,
    secret_ref_manager: Arc<SecretRefManager>,
    /// Workqueue per resource kind
    workqueues: HashMap<&'static str, Arc<Workqueue>>,
    process_config: ProcessConfig,
}

impl FileSystemSyncController {
    /// Create a new FileSystemSyncController
    pub fn new(conf_dir: PathBuf, config_server: Arc<ConfigServer>) -> Self {
        let requeue_registry = Arc::new(RequeueRegistry::new());
        let secret_ref_manager = Arc::new(SecretRefManager::new());

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
            requeue_registry,
            secret_ref_manager,
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
            &self.secret_ref_manager,
        );

        // Process based on kind
        self.process_by_kind(&content, kind, &ctx, true)
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
    ///
    /// Uses filename convention to determine resource identity:
    /// - `Kind_namespace_name.yaml` → key = "namespace/name"
    /// - `Kind__name.yaml` → key = "name" (cluster-scoped)
    async fn handle_file_event(&self, path: &Path) -> Result<()> {
        // Parse resource info from filename
        let Some((kind, namespace, name)) = parse_resource_filename(path) else {
            tracing::trace!(
                component = "fs_sync_controller",
                path = %path.display(),
                "Skipping file: not a valid resource filename"
            );
            return Ok(());
        };

        // Build resource key
        let key = match namespace {
            Some(ns) => format!("{}/{}", ns, name),
            None => name.to_string(),
        };

        // Enqueue to workqueue - worker will determine if it's create/update/delete
        if let Some(queue) = self.workqueues.get(kind) {
            queue.enqueue(key.clone()).await;
            tracing::debug!(
                component = "fs_sync_controller",
                path = %path.display(),
                kind = kind,
                key = %key,
                exists = path.exists(),
                "Enqueued file event"
            );
        } else {
            tracing::trace!(
                component = "fs_sync_controller",
                kind = kind,
                "No workqueue for kind, skipping"
            );
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
        let conf_dir = self.conf_dir.clone();
        let requeue_registry = self.requeue_registry.clone();
        let secret_ref_manager = self.secret_ref_manager.clone();
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
                    &conf_dir,
                    &config_server,
                    &requeue_registry,
                    &secret_ref_manager,
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

/// Parse resource info from filename
///
/// Filename format (matches FileSystemWriter):
/// - With namespace: `{Kind}_{namespace}_{name}.yaml`
/// - Cluster-scoped: `{Kind}__{name}.yaml` (double underscore)
///
/// Returns: (kind, namespace, name)
fn parse_resource_filename(path: &Path) -> Option<(&str, Option<&str>, &str)> {
    let filename = path.file_stem()?.to_str()?;

    // Split into at most 3 parts by underscore
    let parts: Vec<&str> = filename.splitn(3, '_').collect();

    if parts.len() == 3 {
        let kind = parts[0];
        let namespace = if parts[1].is_empty() {
            None // Double underscore means cluster-scoped
        } else {
            Some(parts[1])
        };
        let name = parts[2];
        Some((kind, namespace, name))
    } else {
        None
    }
}

/// Build file path from kind and key
///
/// Key format:
/// - Namespaced: "namespace/name"
/// - Cluster-scoped: "name"
fn build_path_from_key(conf_dir: &Path, kind: &str, key: &str) -> PathBuf {
    let (namespace, name) = if let Some(pos) = key.find('/') {
        (Some(&key[..pos]), &key[pos + 1..])
    } else {
        (None, key)
    };

    let filename = match namespace {
        Some(ns) => format!("{}_{}_{}.yaml", kind, ns, name),
        None => format!("{}__{}.yaml", kind, name),
    };

    conf_dir.join(filename)
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
/// Similar to K8s mode's process_work_item:
/// - File exists → read and process (create/update)
/// - File doesn't exist → get from cache and delete
fn process_work_item(
    kind: &'static str,
    key: &str,
    conf_dir: &Path,
    config_server: &ConfigServer,
    requeue_registry: &RequeueRegistry,
    secret_ref_manager: &SecretRefManager,
    process_config: &ProcessConfig,
) {
    let ctx = ProcessContext::new(
        config_server,
        process_config.metadata_filter.as_ref(),
        None,
        requeue_registry,
        secret_ref_manager,
    );

    // Build path from kind and key
    let path = build_path_from_key(conf_dir, kind, key);

    match kind {
        "GatewayClass" => process_work_item_typed::<GatewayClass, _>(
            key,
            &path,
            &GatewayClassProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "Gateway" => process_work_item_typed::<Gateway, _>(
            key,
            &path,
            &GatewayProcessor::new(None),
            &ctx,
            config_server,
            kind,
        ),
        "HTTPRoute" => process_work_item_typed::<HTTPRoute, _>(
            key,
            &path,
            &HttpRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "GRPCRoute" => process_work_item_typed::<GRPCRoute, _>(
            key,
            &path,
            &GrpcRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "TCPRoute" => process_work_item_typed::<TCPRoute, _>(
            key,
            &path,
            &TcpRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "UDPRoute" => process_work_item_typed::<UDPRoute, _>(
            key,
            &path,
            &UdpRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "TLSRoute" => process_work_item_typed::<TLSRoute, _>(
            key,
            &path,
            &TlsRouteProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "Service" => process_work_item_typed::<Service, _>(
            key,
            &path,
            &ServiceProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "Endpoint" => process_work_item_typed::<Endpoints, _>(
            key,
            &path,
            &EndpointsProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EndpointSlice" => process_work_item_typed::<EndpointSlice, _>(
            key,
            &path,
            &EndpointSliceProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "Secret" => process_work_item_typed::<Secret, _>(
            key,
            &path,
            &SecretProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "ReferenceGrant" => process_work_item_typed::<ReferenceGrant, _>(
            key,
            &path,
            &ReferenceGrantProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "BackendTLSPolicy" => process_work_item_typed::<BackendTLSPolicy, _>(
            key,
            &path,
            &BackendTlsPolicyProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EdgionGatewayConfig" => process_work_item_typed::<EdgionGatewayConfig, _>(
            key,
            &path,
            &EdgionGatewayConfigProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EdgionTls" => process_work_item_typed::<EdgionTls, _>(
            key,
            &path,
            &EdgionTlsProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EdgionPlugins" => process_work_item_typed::<EdgionPlugins, _>(
            key,
            &path,
            &EdgionPluginsProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "EdgionStreamPlugins" => process_work_item_typed::<EdgionStreamPlugins, _>(
            key,
            &path,
            &EdgionStreamPluginsProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "PluginMetaData" => process_work_item_typed::<PluginMetaData, _>(
            key,
            &path,
            &PluginMetadataProcessor::new(),
            &ctx,
            config_server,
            kind,
        ),
        "LinkSys" => process_work_item_typed::<LinkSys, _>(
            key,
            &path,
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
///
/// Logic mirrors K8s mode:
/// - File exists → read, parse, process_resource (create/update)
/// - File doesn't exist + cache has it → process_resource_delete
/// - File doesn't exist + no cache → already deleted, skip
fn process_work_item_typed<K, P>(
    key: &str,
    path: &Path,
    processor: &P,
    ctx: &ProcessContext,
    config_server: &ConfigServer,
    kind: &'static str,
) where
    K: Resource + Clone + Send + Sync + DeserializeOwned + 'static,
    P: ResourceProcessor<K>,
{
    if path.exists() {
        // File exists → read and process
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
    } else {
        // File doesn't exist → check cache for deletion
        if let Some(cached) = processor.get(config_server, key) {
            process_resource_delete(cached, processor, ctx, kind);
        } else {
            tracing::trace!(
                component = "fs_sync_controller",
                kind = kind,
                key = key,
                "Resource not found in file or cache, skipping"
            );
        }
    }
}
