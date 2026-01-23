//! Configuration Center (ConfCenter)
//!
//! Unified configuration management supporting multiple backends:
//! - FileSystem: Local YAML files with unified sync controller
//! - Kubernetes: K8s API with kube-runtime Controller-based resource watching
//!
//! Architecture:
//! ```text
//! ConfCenter
//! ├── FileSystem Mode
//! │   ├── FileSystemWriter (ConfWriter impl) - read/write local files
//! │   └── FileSystemSyncController - unified init + runtime with workqueue
//! │       ├── Init phase: scan directory, process directly (no queue)
//! │       └── Runtime phase: file watcher → workqueue → ResourceProcessor
//! └── Kubernetes Mode
//!     ├── KubernetesWriter (ConfWriter impl) - call K8s API
//!     ├── KubernetesController - kube-runtime Controller pattern (includes leader election)
//!     │   └── ResourceController per resource type with workqueue
//!     └── ResourceStores - reflector::Store for each resource type
//!
//! Common Components (sync_runtime module):
//! ├── Workqueue - Go operator-style work queue with dedup/retry/backoff
//! ├── ResourceProcessor - trait for resource-specific processing with validation
//! ├── RequeueRegistry - cross-resource requeue mechanism
//! ├── ShutdownSignal/Handle - graceful shutdown coordination
//! └── Metrics - controller and resource count metrics
//! ```
//!
//! ## Lifecycle Management
//!
//! ConfCenter uses `start()` which dispatches to mode-specific lifecycle methods:
//!
//! - **FileSystem mode** (`lifecycle_filesystem.rs`):
//!   1. Create ConfigServer with configured endpoint_mode
//!   2. Run FileSystemSyncController (init + runtime phases)
//!   3. Set config_server = Some (services become available)
//!   4. Block until shutdown signal
//!
//! - **K8s mode** (`lifecycle_kubernetes.rs`): Event-driven with leader election
//!   1. Initialize leader election
//!   2. Wait for leadership
//!   3. Start event watchers (controller, caches, leadership)
//!   4. Event loop until shutdown or error
//!   5. On leadership loss, restart from step 2
//!
//! ConfigServer is `Option<Arc<ConfigServer>>`:
//! - None: Not ready (startup, restart, leadership loss)
//! - Some: Ready to serve requests
//!
//! gRPC and Admin services get ConfigServer via `config_server()` method.
//! When None, they return UNAVAILABLE errors.

mod config;
pub mod file_system;
pub mod kubernetes;
mod lifecycle_filesystem;
mod lifecycle_kubernetes;
pub mod status;
pub mod sync_runtime;
pub mod traits;

pub use config::{ConfCenterConfig, EndpointMode, LeaderElectionConfig, MetadataFilterConfig};
pub use file_system::{FileSystemSyncController, FileSystemWriter};
pub use kubernetes::{
    ControllerExitReason, KubernetesController, KubernetesStatusStore, KubernetesWriter, NamespaceWatchMode,
    RelinkReason, StatusStore, StatusStoreError,
};
pub use status::FileSystemStatusStore;
pub use traits::{ConfEntry, ConfWriter, ConfWriterError, ListOptions, ListResult};

use crate::core::cli::config::{ConfSyncConfig, EdgionControllerConfig};
use crate::core::conf_sync::ConfigServer;
use anyhow::Result;
use sync_runtime::ShutdownHandle;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// ConfCenter - Unified configuration center
///
/// Provides a unified interface for configuration management regardless of backend.
/// Manages ConfigServer lifecycle internally - ConfigServer is `Option<Arc<ConfigServer>>`:
/// - None: System not ready (startup, restart, leadership loss)
/// - Some: System ready to serve requests
///
/// ## Lifecycle
///
/// The `start()` method dispatches to mode-specific lifecycle methods:
/// - FileSystem: `lifecycle_filesystem.rs` - Simple one-shot setup, then block
/// - K8s: `lifecycle_kubernetes.rs` - Loop with automatic restart on failure
///
/// gRPC and Admin services get ConfigServer via `config_server()` method.
/// When None, they should return UNAVAILABLE errors.
pub struct ConfCenter {
    config: ConfCenterConfig,
    conf_sync_config: ConfSyncConfig,
    writer: Arc<dyn ConfWriter>,

    /// ConfigServer instance - Option to support lifecycle management
    /// None: Not ready (startup, restart, leadership loss)
    /// Some: Ready to serve requests
    config_server: RwLock<Option<Arc<ConfigServer>>>,

    // ==================== FileSystem Mode Fields ====================
    /// Shutdown handle for stopping sync tasks
    shutdown_handle: Mutex<Option<ShutdownHandle>>,
    /// Handle to the running watcher task
    watcher_handle: Mutex<Option<JoinHandle<()>>>,
    // ==================== Kubernetes Mode Fields ====================
    // (Currently managed internally by KubernetesController)

    // ==================== Future: Etcd Mode Fields ====================
    // etcd_client: Mutex<Option<...>>,
    // etcd_watch_handle: Mutex<Option<...>>,
}

impl ConfCenter {
    /// Create a new ConfCenter based on configuration
    ///
    /// Note: ConfigServer is NOT created here. It will be created in `start()` method.
    pub async fn create(config: &EdgionControllerConfig) -> Result<Self> {
        let conf_center_config = config.conf_center.clone();
        let conf_sync_config = config.conf_sync.clone();

        match &conf_center_config {
            ConfCenterConfig::FileSystem { conf_dir, .. } => {
                tracing::info!(
                    component = "conf_center",
                    mode = "file_system",
                    conf_dir = %conf_dir.display(),
                    "Creating FileSystem ConfCenter"
                );
                let writer = FileSystemWriter::new(conf_dir);
                Ok(Self {
                    config: conf_center_config,
                    conf_sync_config,
                    writer: Arc::new(writer),
                    config_server: RwLock::new(None),
                    shutdown_handle: Mutex::new(None),
                    watcher_handle: Mutex::new(None),
                })
            }
            ConfCenterConfig::Kubernetes { .. } => {
                tracing::info!(
                    component = "conf_center",
                    mode = "kubernetes",
                    "Creating Kubernetes ConfCenter"
                );
                let writer = KubernetesWriter::new().await?;
                Ok(Self {
                    config: conf_center_config,
                    conf_sync_config,
                    writer: Arc::new(writer),
                    config_server: RwLock::new(None),
                    shutdown_handle: Mutex::new(None),
                    watcher_handle: Mutex::new(None),
                })
            }
        }
    }

    // ==================== Lifecycle Management ====================

    /// Start the configuration center with an external shutdown handle
    ///
    /// This is the preferred method when the caller manages signal handling.
    /// The shutdown handle will be used to coordinate graceful shutdown.
    ///
    /// Dispatches to mode-specific lifecycle methods:
    /// - FileSystem: `run_filesystem_lifecycle_with_shutdown()` in `lifecycle_filesystem.rs`
    /// - K8s: `run_k8s_lifecycle_with_shutdown()` in `lifecycle_kubernetes.rs`
    pub async fn start_with_shutdown(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        if self.is_k8s_mode() {
            self.run_k8s_lifecycle_with_shutdown(shutdown_handle).await
        } else {
            self.run_filesystem_lifecycle_with_shutdown(shutdown_handle).await
        }
    }

    /// Start the configuration center (creates its own shutdown handler)
    ///
    /// This method creates its own ShutdownHandle and signal listener.
    /// Use `start_with_shutdown()` if the caller manages signal handling.
    ///
    /// Dispatches to mode-specific lifecycle methods:
    /// - FileSystem: `run_filesystem_lifecycle()` in `lifecycle_filesystem.rs`
    /// - K8s: `run_k8s_lifecycle()` in `lifecycle_kubernetes.rs`
    pub async fn start(&self) -> Result<()> {
        // Create shutdown handle and start signal listener
        let shutdown_handle = ShutdownHandle::new();
        let signal_shutdown = shutdown_handle.clone();
        tokio::spawn(async move {
            signal_shutdown.wait_for_signals().await;
        });

        self.start_with_shutdown(shutdown_handle).await
    }

    // ==================== Helper Methods ====================

    /// Wait for all caches to be ready
    pub(crate) async fn wait_caches_ready(&self, config_server: &Arc<ConfigServer>, timeout_secs: u64) {
        let start = Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if config_server.is_each_cache_ready() {
                tracing::info!(
                    component = "conf_center",
                    event = "all_caches_ready",
                    elapsed_ms = start.elapsed().as_millis(),
                    "All caches are ready"
                );
                return;
            }

            if start.elapsed() > timeout {
                let not_ready = config_server.not_ready_caches();
                tracing::warn!(
                    component = "conf_center",
                    event = "wait_caches_timeout",
                    timeout_secs = timeout_secs,
                    not_ready = ?not_ready,
                    "Timeout waiting for caches, proceeding anyway"
                );
                return;
            }

            let not_ready = config_server.not_ready_caches();
            tracing::debug!(
                component = "conf_center",
                event = "waiting_for_caches",
                not_ready = ?not_ready,
                elapsed_ms = start.elapsed().as_millis(),
                "Waiting for caches to be ready..."
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Set the ConfigServer (Some = ready, None = not ready)
    pub(crate) fn set_config_server(&self, server: Option<Arc<ConfigServer>>) {
        let mut config_server = self.config_server.write().unwrap();
        let was_ready = config_server.is_some();
        let is_ready = server.is_some();
        *config_server = server;

        if was_ready != is_ready {
            tracing::info!(
                component = "conf_center",
                event = "config_server_state_changed",
                was_ready = was_ready,
                is_ready = is_ready,
                "ConfigServer state changed"
            );
        }
    }

    // ==================== Public API ====================

    /// Get the configuration writer
    pub fn writer(&self) -> Arc<dyn ConfWriter> {
        self.writer.clone()
    }

    /// Get the ConfigServer (may be None if not ready)
    ///
    /// gRPC and Admin services should call this method to get the ConfigServer.
    /// When None, they should return UNAVAILABLE/NOT_READY errors.
    pub fn config_server(&self) -> Option<Arc<ConfigServer>> {
        self.config_server.read().unwrap().clone()
    }

    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        self.config.is_k8s_mode()
    }

    /// Get the configuration
    pub fn config(&self) -> &ConfCenterConfig {
        &self.config
    }

    /// Check if the system is ready
    ///
    /// Ready means ConfigServer exists and can serve requests.
    pub fn is_ready(&self) -> bool {
        self.config_server.read().unwrap().is_some()
    }

    /// Reload all resources (FileSystem mode only)
    ///
    /// Performs a complete reset:
    /// 1. Clear all caches (remove stale data from deleted files)
    /// 2. Run init_phase to reload from directory (two passes for dependency resolution)
    ///
    /// This ensures that deleted files are properly removed from the cache,
    /// unlike the old `load_all_resources` which only added resources incrementally.
    ///
    /// Note: Two passes are needed because resources are processed in filename order,
    /// and EdgionTls/Gateway depend on Secret which comes later alphabetically.
    /// First pass loads all resources, second pass resolves Secret references.
    pub async fn reload(&self) -> Result<()> {
        if self.is_k8s_mode() {
            return Err(anyhow::anyhow!("Reload not supported in K8s mode"));
        }

        let ConfCenterConfig::FileSystem { conf_dir, .. } = &self.config else {
            return Err(anyhow::anyhow!("Not in FileSystem mode"));
        };

        let config_server = self
            .config_server()
            .ok_or_else(|| anyhow::anyhow!("ConfigServer not available"))?;

        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            conf_dir = %conf_dir.display(),
            "Reloading all resources (full reset)"
        );

        // 1. Clear all caches (handles deleted files)
        config_server.clear_all_caches();

        // 2. Run init_phase twice to ensure Secret dependencies are resolved
        // First pass: load all resources (EdgionTls/Gateway may not find Secrets yet)
        // Second pass: re-process to resolve Secret references
        let controller = FileSystemSyncController::new_for_reload(conf_dir.clone(), config_server);
        controller.init_phase().await?;

        tracing::debug!(
            component = "conf_center",
            mode = "file_system",
            "First pass complete, running second pass for Secret dependency resolution"
        );

        // Second pass to resolve Secret dependencies
        controller.init_phase().await?;

        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "Reload complete"
        );

        Ok(())
    }
}
