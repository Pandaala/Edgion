//! FileSystemCenter - Unified configuration center for FileSystem mode
//!
//! Implements both CenterApi (CRUD) and CenterLifeCycle (lifecycle management),
//! automatically getting ConfCenter via blanket impl.
//!
//! ## Architecture
//!
//! ```text
//! FileSystemCenter
//! ├── writer: FileSystemWriter (CRUD delegate)
//! ├── config: ConfCenterConfig
//! ├── config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>
//! ├── shutdown_handle: Mutex<Option<ShutdownHandle>>
//! └── controller_handle: Mutex<Option<JoinHandle<()>>>
//! ```

use super::controller::FileSystemController;
use super::writer::FileSystemWriter;
use crate::core::conf_mgr_new::config::{ConfCenterConfig, EndpointMode};
use crate::core::conf_mgr_new::conf_center::traits::{
    CenterApi, CenterLifeCycle, ConfWriterError, ListOptions, ListResult,
};
use crate::core::conf_mgr_new::sync_runtime::ShutdownHandle;
use crate::core::conf_mgr_new::PROCESSOR_REGISTRY;
use crate::core::conf_sync::conf_server_new::ConfigSyncServer;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// FileSystemCenter - Configuration center for FileSystem mode
///
/// This struct implements both `CenterApi` and `CenterLifeCycle`,
/// automatically getting `ConfCenter` implementation via blanket impl.
pub struct FileSystemCenter {
    /// Configuration
    config: ConfCenterConfig,
    /// Writer for CRUD operations (delegate)
    writer: FileSystemWriter,
    /// ConfigSyncServer instance for gRPC list/watch
    /// None: Not ready (startup, restart)
    /// Some: Ready to serve requests
    config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>,
    /// Shutdown handle for stopping sync tasks
    shutdown_handle: Mutex<Option<ShutdownHandle>>,
    /// Handle to the running controller task
    controller_handle: Mutex<Option<JoinHandle<()>>>,
}

impl FileSystemCenter {
    /// Create a new FileSystemCenter
    pub fn new(config: ConfCenterConfig) -> Result<Self> {
        let ConfCenterConfig::FileSystem { conf_dir, .. } = &config else {
            return Err(anyhow::anyhow!("FileSystemCenter requires FileSystem config"));
        };

        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            conf_dir = %conf_dir.display(),
            "Creating FileSystemCenter"
        );

        let writer = FileSystemWriter::new(conf_dir);

        Ok(Self {
            config,
            writer,
            config_sync_server: RwLock::new(None),
            shutdown_handle: Mutex::new(None),
            controller_handle: Mutex::new(None),
        })
    }

    /// Get the configuration
    pub fn config(&self) -> &ConfCenterConfig {
        &self.config
    }

    // ==================== Helper Methods ====================

    /// Set the ConfigSyncServer (Some = ready, None = not ready)
    fn set_config_sync_server(&self, server: Option<Arc<ConfigSyncServer>>) {
        let mut sync_server = self.config_sync_server.write().unwrap();
        let was_ready = sync_server.is_some();
        let is_ready = server.is_some();
        *sync_server = server;

        if was_ready != is_ready {
            tracing::info!(
                component = "file_system_center",
                event = "config_sync_server_state_changed",
                was_ready = was_ready,
                is_ready = is_ready,
                "ConfigSyncServer state changed"
            );
        }
    }

    /// Store shutdown handle for lifecycle management
    fn set_shutdown_handle(&self, handle: ShutdownHandle) {
        let mut shutdown_handle = self.shutdown_handle.lock().unwrap();
        *shutdown_handle = Some(handle);
    }

    /// Get shutdown signal from stored handle
    fn get_shutdown_signal(&self) -> Option<crate::core::conf_mgr_new::sync_runtime::ShutdownSignal> {
        let handle = self.shutdown_handle.lock().unwrap();
        handle.as_ref().map(|h| h.signal())
    }

    /// Store controller task handle for cleanup
    fn set_controller_handle(&self, handle: JoinHandle<()>) {
        let mut controller_handle = self.controller_handle.lock().unwrap();
        *controller_handle = Some(handle);
    }

    /// Abort and take controller handle
    fn abort_controller(&self) {
        if let Some(handle) = self.controller_handle.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Wait for PROCESSOR_REGISTRY to be ready
    async fn wait_registry_ready(&self, timeout_secs: u64) {
        let start = Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if PROCESSOR_REGISTRY.is_all_ready() {
                tracing::info!(
                    component = "file_system_center",
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
                    component = "file_system_center",
                    event = "wait_registry_timeout",
                    timeout_secs = timeout_secs,
                    not_ready = ?not_ready,
                    "Timeout waiting for processors, proceeding anyway"
                );
                return;
            }

            tracing::debug!(
                component = "file_system_center",
                event = "waiting_for_processors",
                elapsed_ms = start.elapsed().as_millis(),
                "Waiting for processors to be ready..."
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

// ============================================================================
// CenterApi implementation - delegates to FileSystemWriter
// ============================================================================

#[async_trait]
impl CenterApi for FileSystemCenter {
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        self.writer.set_one(kind, namespace, name, content).await
    }

    async fn create_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        self.writer.create_one(kind, namespace, name, content).await
    }

    async fn update_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        self.writer.update_one(kind, namespace, name, content).await
    }

    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfWriterError> {
        self.writer.get_one(kind, namespace, name).await
    }

    async fn get_list_by_kind(&self, kind: &str, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        self.writer.get_list_by_kind(kind, opts).await
    }

    async fn get_list_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        opts: Option<ListOptions>,
    ) -> Result<ListResult, ConfWriterError> {
        self.writer.get_list_by_kind_ns(kind, namespace, opts).await
    }

    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfWriterError> {
        self.writer.cnt_by_kind(kind).await
    }

    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfWriterError> {
        self.writer.cnt_by_kind_ns(kind, namespace).await
    }

    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
        self.writer.delete_one(kind, namespace, name).await
    }

    async fn list_all(&self, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        self.writer.list_all(opts).await
    }
}

// ============================================================================
// CenterLifeCycle implementation - FileSystem lifecycle logic
// ============================================================================

#[async_trait]
impl CenterLifeCycle for FileSystemCenter {
    /// FileSystem mode lifecycle with external shutdown handle
    ///
    /// Uses the new FileSystemController for unified init + runtime flow:
    /// 1. Resolve endpoint mode
    /// 2. Run FileSystemController (registers to PROCESSOR_REGISTRY, runs init + runtime)
    /// 3. Wait for PROCESSOR_REGISTRY to be ready
    /// 4. Create ConfigSyncServer and register WatchObjs
    /// 5. Set config_sync_server = Some (services become available)
    /// 6. Wait for shutdown signal
    async fn start(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            "Starting FileSystem lifecycle"
        );

        // Store shutdown handle for external access
        self.set_shutdown_handle(shutdown_handle.clone());

        let shutdown_signal = shutdown_handle.signal();

        // 1. Get configuration
        let ConfCenterConfig::FileSystem { conf_dir, .. } = self.config() else {
            return Err(anyhow::anyhow!("Not in FileSystem mode"));
        };

        // 2. Resolve endpoint mode (Auto -> EndpointSlice in FileSystem mode)
        let endpoint_mode = match self.config().endpoint_mode() {
            EndpointMode::Auto => EndpointMode::EndpointSlice,
            mode => mode,
        };

        crate::core::backends::init_global_endpoint_mode(endpoint_mode);

        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            endpoint_mode = ?endpoint_mode,
            conf_dir = %conf_dir.display(),
            "Using endpoint mode"
        );

        // 3. Create error channel for controller errors
        let (error_tx, error_rx) = oneshot::channel::<String>();

        // 4. Create and spawn FileSystemController
        let controller = FileSystemController::new(conf_dir.clone(), endpoint_mode);
        let controller_shutdown = shutdown_handle.signal();

        let handle = tokio::spawn(async move {
            if let Err(e) = controller.run(controller_shutdown).await {
                let error_msg = e.to_string();
                tracing::error!(
                    component = "file_system_center",
                    mode = "file_system",
                    error = %error_msg,
                    "FileSystemController error"
                );
                let _ = error_tx.send(error_msg);
            }
        });

        self.set_controller_handle(handle);

        // 5. Wait for PROCESSOR_REGISTRY to be ready (with timeout)
        self.wait_registry_ready(30).await;

        // 6. Create ConfigSyncServer and register all WatchObjs
        let config_sync_server = Arc::new(ConfigSyncServer::new());
        config_sync_server.set_endpoint_mode(endpoint_mode);
        config_sync_server.register_all(PROCESSOR_REGISTRY.all_watch_objs());

        // 7. Set config_sync_server = Some (services become available)
        self.set_config_sync_server(Some(config_sync_server));

        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            "ConfigSyncServer is ready, gRPC services can process requests"
        );

        // 8. Wait for shutdown signal or controller error
        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            "FileSystem mode: running until shutdown signal"
        );

        let mut shutdown_signal = shutdown_signal;
        let mut error_rx = error_rx;

        tokio::select! {
            _ = shutdown_signal.wait() => {
                tracing::info!(
                    component = "file_system_center",
                    mode = "file_system",
                    "Received shutdown signal"
                );
            }
            result = &mut error_rx => {
                if let Ok(error_msg) = result {
                    tracing::error!(
                        component = "file_system_center",
                        mode = "file_system",
                        error = %error_msg,
                        "Controller stopped with error"
                    );
                    // Continue waiting for shutdown - controller error is not fatal
                    // User can still use cached data
                    shutdown_signal.wait().await;
                }
            }
        }

        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            "Cleaning up"
        );

        // 9. Cleanup
        self.set_config_sync_server(None);
        self.abort_controller();

        // 10. Clear PROCESSOR_REGISTRY
        PROCESSOR_REGISTRY.clear_registry();

        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            "FileSystem lifecycle completed"
        );

        Ok(())
    }

    /// Reload all resources (FileSystem mode)
    ///
    /// Performs a complete reset:
    /// 1. Clear all caches in PROCESSOR_REGISTRY
    /// 2. Set all processors to not ready
    /// 3. Run FileSystemController init phase
    async fn reload(&self) -> Result<()> {
        let ConfCenterConfig::FileSystem { conf_dir, .. } = &self.config else {
            return Err(anyhow::anyhow!("Not in FileSystem mode"));
        };

        tracing::info!(
            component = "file_system_center",
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
            component = "file_system_center",
            mode = "file_system",
            "Reload complete"
        );

        Ok(())
    }

    /// Check if the system is ready
    fn is_ready(&self) -> bool {
        PROCESSOR_REGISTRY.is_all_ready() && self.config_sync_server.read().unwrap().is_some()
    }

    /// Get the ConfigSyncServer (may be None if not ready)
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        self.config_sync_server.read().unwrap().clone()
    }

    /// Check if running in Kubernetes mode
    fn is_k8s_mode(&self) -> bool {
        false
    }
}

// FileSystemCenter automatically implements ConfCenter via blanket impl
// because it implements both CenterApi and CenterLifeCycle
