//! ConfCenter - Unified configuration center (new architecture)
//!
//! This is the new implementation using PROCESSOR_REGISTRY instead of ConfigServer.
//!
//! ## Architecture
//!
//! ```text
//! ConfCenter
//! ├── PROCESSOR_REGISTRY (global, holds Arc<dyn ProcessorObj>)
//! │   └── ResourceProcessor<T> for each resource type
//! ├── ConfigSyncServer (for gRPC list/watch)
//! │   └── Uses WatchObj from PROCESSOR_REGISTRY
//! └── ConfWriter (for Admin API CRUD)
//! ```
//!
//! ## Lifecycle
//!
//! - FileSystem mode: `lifecycle_filesystem.rs`
//! - Kubernetes mode: `lifecycle_kubernetes.rs`

use super::config::ConfCenterConfig;
use super::file_system::{FileSystemController, FileSystemWriter};
use super::kubernetes::KubernetesWriter;
use super::traits::ConfWriter;
use crate::core::conf_mgr_new::sync_runtime::ShutdownHandle;
use crate::core::conf_mgr_new::PROCESSOR_REGISTRY;
use crate::core::conf_sync::conf_server_new::ConfigSyncServer;
use anyhow::Result;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// ConfCenter - Unified configuration center
///
/// Provides a unified interface for configuration management regardless of backend.
/// Uses PROCESSOR_REGISTRY for resource caching and ConfigSyncServer for gRPC.
///
/// ## State Management
///
/// - `config_sync_server: Option<Arc<ConfigSyncServer>>`:
///   - None: System not ready (startup, restart, leadership loss)
///   - Some: System ready to serve requests
///
/// gRPC services get ConfigSyncServer via `config_sync_server()` method.
/// When None, they should return UNAVAILABLE errors.
pub struct ConfCenter {
    config: ConfCenterConfig,
    writer: Arc<dyn ConfWriter>,

    /// ConfigSyncServer instance for gRPC list/watch
    /// None: Not ready (startup, restart, leadership loss)
    /// Some: Ready to serve requests
    config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>,

    /// Shutdown handle for stopping sync tasks
    shutdown_handle: Mutex<Option<ShutdownHandle>>,
    /// Handle to the running controller task
    controller_handle: Mutex<Option<JoinHandle<()>>>,
}

impl ConfCenter {
    /// Create a new ConfCenter based on configuration
    ///
    /// Note: ConfigSyncServer is NOT created here. It will be created in `start()` method
    /// after PROCESSOR_REGISTRY is populated.
    pub async fn create(config: ConfCenterConfig) -> Result<Self> {
        match &config {
            ConfCenterConfig::FileSystem { conf_dir, .. } => {
                tracing::info!(
                    component = "conf_center_new",
                    mode = "file_system",
                    conf_dir = %conf_dir.display(),
                    "Creating FileSystem ConfCenter"
                );
                let writer = FileSystemWriter::new(conf_dir);
                Ok(Self {
                    config,
                    writer: Arc::new(writer),
                    config_sync_server: RwLock::new(None),
                    shutdown_handle: Mutex::new(None),
                    controller_handle: Mutex::new(None),
                })
            }
            ConfCenterConfig::Kubernetes { .. } => {
                tracing::info!(
                    component = "conf_center_new",
                    mode = "kubernetes",
                    "Creating Kubernetes ConfCenter"
                );
                let writer = KubernetesWriter::new().await?;
                Ok(Self {
                    config,
                    writer: Arc::new(writer),
                    config_sync_server: RwLock::new(None),
                    shutdown_handle: Mutex::new(None),
                    controller_handle: Mutex::new(None),
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
    /// - Kubernetes: `run_k8s_lifecycle_with_shutdown()` in `lifecycle_kubernetes.rs`
    pub async fn start_with_shutdown(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        if self.is_k8s_mode() {
            self.run_k8s_lifecycle_with_shutdown(shutdown_handle).await
        } else {
            self.run_filesystem_lifecycle_with_shutdown(shutdown_handle)
                .await
        }
    }

    /// Start the configuration center (creates its own shutdown handler)
    ///
    /// This method creates its own ShutdownHandle and signal listener.
    /// Use `start_with_shutdown()` if the caller manages signal handling.
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

    /// Wait for PROCESSOR_REGISTRY to be ready
    pub(crate) async fn wait_registry_ready(&self, timeout_secs: u64) {
        let start = Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if PROCESSOR_REGISTRY.is_all_ready() {
                tracing::info!(
                    component = "conf_center_new",
                    event = "all_processors_ready",
                    elapsed_ms = start.elapsed().as_millis(),
                    "All processors are ready"
                );
                return;
            }

            if start.elapsed() > timeout {
                let not_ready = PROCESSOR_REGISTRY
                    .all_kinds()
                    .into_iter()
                    .filter(|kind| {
                        PROCESSOR_REGISTRY
                            .get(kind)
                            .map(|p| !p.is_ready())
                            .unwrap_or(false)
                    })
                    .collect::<Vec<_>>();

                tracing::warn!(
                    component = "conf_center_new",
                    event = "wait_registry_timeout",
                    timeout_secs = timeout_secs,
                    not_ready = ?not_ready,
                    "Timeout waiting for processors, proceeding anyway"
                );
                return;
            }

            tracing::debug!(
                component = "conf_center_new",
                event = "waiting_for_processors",
                elapsed_ms = start.elapsed().as_millis(),
                "Waiting for processors to be ready..."
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Set the ConfigSyncServer (Some = ready, None = not ready)
    pub(crate) fn set_config_sync_server(&self, server: Option<Arc<ConfigSyncServer>>) {
        let mut sync_server = self.config_sync_server.write().unwrap();
        let was_ready = sync_server.is_some();
        let is_ready = server.is_some();
        *sync_server = server;

        if was_ready != is_ready {
            tracing::info!(
                component = "conf_center_new",
                event = "config_sync_server_state_changed",
                was_ready = was_ready,
                is_ready = is_ready,
                "ConfigSyncServer state changed"
            );
        }
    }

    /// Store shutdown handle for lifecycle management
    pub(crate) fn set_shutdown_handle(&self, handle: ShutdownHandle) {
        let mut shutdown_handle = self.shutdown_handle.lock().unwrap();
        *shutdown_handle = Some(handle);
    }

    /// Get shutdown signal from stored handle
    pub(crate) fn get_shutdown_signal(&self) -> Option<crate::core::conf_mgr_new::sync_runtime::ShutdownSignal> {
        let handle = self.shutdown_handle.lock().unwrap();
        handle.as_ref().map(|h| h.signal())
    }

    /// Store controller task handle for cleanup
    pub(crate) fn set_controller_handle(&self, handle: JoinHandle<()>) {
        let mut controller_handle = self.controller_handle.lock().unwrap();
        *controller_handle = Some(handle);
    }

    /// Abort and take controller handle
    pub(crate) fn abort_controller(&self) {
        if let Some(handle) = self.controller_handle.lock().unwrap().take() {
            handle.abort();
        }
    }

    // ==================== Public API ====================

    /// Get the configuration writer
    pub fn writer(&self) -> Arc<dyn ConfWriter> {
        self.writer.clone()
    }

    /// Get the ConfigSyncServer (may be None if not ready)
    ///
    /// gRPC services should call this method to get the ConfigSyncServer.
    /// When None, they should return UNAVAILABLE/NOT_READY errors.
    pub fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        self.config_sync_server.read().unwrap().clone()
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
    /// Ready means PROCESSOR_REGISTRY is ready and ConfigSyncServer exists.
    pub fn is_ready(&self) -> bool {
        PROCESSOR_REGISTRY.is_all_ready() && self.config_sync_server.read().unwrap().is_some()
    }

    /// Reload all resources (FileSystem mode only)
    ///
    /// Performs a complete reset:
    /// 1. Clear all caches in PROCESSOR_REGISTRY
    /// 2. Set all processors to not ready
    /// 3. Run FileSystemController init phase
    pub async fn reload(&self) -> Result<()> {
        if self.is_k8s_mode() {
            return Err(anyhow::anyhow!("Reload not supported in K8s mode"));
        }

        let ConfCenterConfig::FileSystem { conf_dir, .. } = &self.config else {
            return Err(anyhow::anyhow!("Not in FileSystem mode"));
        };

        tracing::info!(
            component = "conf_center_new",
            mode = "file_system",
            conf_dir = %conf_dir.display(),
            "Reloading all resources (full reset)"
        );

        // 1. Clear all caches and set not ready
        PROCESSOR_REGISTRY.clear_all();
        PROCESSOR_REGISTRY.set_all_not_ready();

        // 2. Get shutdown signal for the controller
        let shutdown_signal = self
            .get_shutdown_signal()
            .ok_or_else(|| anyhow::anyhow!("Shutdown handle not set"))?;

        // 3. Run a new FileSystemController to reload
        // Note: This is a simplified reload - it re-runs init phase
        let endpoint_mode = self.config.endpoint_mode();
        let controller = FileSystemController::new(conf_dir.clone(), endpoint_mode);

        // Run controller (this will re-register processors and load data)
        controller.run(shutdown_signal).await?;

        tracing::info!(
            component = "conf_center_new",
            mode = "file_system",
            "Reload complete"
        );

        Ok(())
    }
}
