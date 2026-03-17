//! FileSystemCenter - Unified configuration center for FileSystem mode
//!
//! Implements both CenterApi (CRUD) and CenterLifeCycle (lifecycle management),
//! automatically getting ConfCenter via blanket impl.
//!
//! ## Architecture
//!
//! ```text
//! FileSystemCenter
//! ├── writer: FileSystemStorage (CRUD delegate)
//! ├── config: FileSystemConfig
//! ├── config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>
//! ├── shutdown_handle: Mutex<Option<ShutdownHandle>>
//! └── controller_handle: Mutex<Option<JoinHandle<()>>>
//! ```

use super::super::common::EndpointMode;
use super::config::FileSystemConfig;
use super::controller::FileSystemController;
use super::storage::FileSystemStorage;
use crate::core::controller::conf_mgr::conf_center::traits::{
    CenterApi, CenterLifeCycle, ConfWriterError, ListOptions, ListResult,
};
use crate::core::controller::conf_mgr::sync_runtime::metrics::reload_metrics;
use crate::core::controller::conf_mgr::sync_runtime::ShutdownHandle;
use crate::core::controller::conf_mgr::PROCESSOR_REGISTRY;
use crate::core::controller::conf_sync::conf_server::ConfigSyncServer;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// FileSystemCenter - Configuration center for FileSystem mode
///
/// This struct implements both `CenterApi` and `CenterLifeCycle`,
/// automatically getting `ConfCenter` implementation via blanket impl.
pub struct FileSystemCenter {
    /// Configuration
    config: FileSystemConfig,
    /// Writer for CRUD operations (delegate)
    writer: FileSystemStorage,
    /// ConfigSyncServer instance for gRPC list/watch
    /// None: Not ready (startup, restart)
    /// Some: Ready to serve requests
    config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>,
    /// Shutdown handle for stopping sync tasks
    shutdown_handle: Mutex<Option<ShutdownHandle>>,
    /// Handle to the running controller task
    controller_handle: Mutex<Option<JoinHandle<()>>>,
    /// Reload signal sender (for triggering reload via Admin API)
    reload_tx: Mutex<Option<mpsc::Sender<()>>>,
}

impl FileSystemCenter {
    /// Create a new FileSystemCenter
    pub fn new(config: FileSystemConfig) -> Result<Self> {
        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            conf_dir = %config.conf_dir().display(),
            "Creating FileSystemCenter"
        );

        let writer = FileSystemStorage::new(config.conf_dir());

        Ok(Self {
            config,
            writer,
            config_sync_server: RwLock::new(None),
            shutdown_handle: Mutex::new(None),
            controller_handle: Mutex::new(None),
            reload_tx: Mutex::new(None),
        })
    }

    /// Get the configuration
    pub fn config(&self) -> &FileSystemConfig {
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

    /// Set reload signal sender
    fn set_reload_tx(&self, tx: Option<mpsc::Sender<()>>) {
        *self.reload_tx.lock().unwrap() = tx;
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
                    .filter(|kind| PROCESSOR_REGISTRY.get(kind).map(|p| !p.is_ready()).unwrap_or(false))
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
// CenterApi implementation - delegates to FileSystemStorage
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
    /// 6. Wait for shutdown signal or reload request
    ///
    /// Supports reload: when reload is requested, the controller is restarted
    /// and a new ConfigSyncServer is created (with new server_id).
    async fn start(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            "Starting FileSystem lifecycle"
        );

        // Store shutdown handle for external access
        self.set_shutdown_handle(shutdown_handle.clone());

        // Get configuration
        let conf_dir = self.config.conf_dir();

        // Resolve endpoint mode:
        // - test_mode: force Both (sync both Endpoints and EndpointSlice)
        // - Auto: defaults to EndpointSlice in FileSystem mode
        // - Others: use as configured
        let endpoint_mode = if crate::core::common::config::is_test_mode() {
            EndpointMode::Both
        } else {
            match self.config.endpoint_mode() {
                EndpointMode::Auto => EndpointMode::EndpointSlice,
                mode => mode,
            }
        };

        crate::core::gateway::backends::init_global_endpoint_mode(endpoint_mode);

        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            endpoint_mode = ?endpoint_mode,
            test_mode = crate::core::common::config::is_test_mode(),
            conf_dir = %conf_dir.display(),
            "Using endpoint mode"
        );

        // Track reload state for metrics
        let mut reload_start_time: Option<Instant> = None;

        // Outer loop to support reload
        loop {
            // Record reload completion time if this is a reload iteration
            if let Some(start_time) = reload_start_time.take() {
                let duration = start_time.elapsed().as_secs_f64();
                reload_metrics().reload_completed(duration);
                tracing::info!(
                    component = "file_system_center",
                    duration_secs = duration,
                    "Reload completed"
                );
            }

            // 1. Create iteration-specific shutdown handle for controller
            let iteration_shutdown = ShutdownHandle::new();

            // 2. Create reload channel
            let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);
            self.set_reload_tx(Some(reload_tx));

            // 3. Create error channel for controller errors
            let (error_tx, error_rx) = oneshot::channel::<String>();

            // 4. Create and spawn FileSystemController (using iteration_shutdown)
            let controller = FileSystemController::new(conf_dir.clone(), endpoint_mode);
            let controller_shutdown = iteration_shutdown.signal();

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

            // 5.5. Trigger full cross-namespace revalidation
            crate::core::controller::conf_mgr::sync_runtime::resource_processor::trigger_full_cross_ns_revalidation();
            crate::core::controller::conf_mgr::sync_runtime::resource_processor::trigger_gateway_secret_revalidation();
            crate::core::controller::conf_mgr::sync_runtime::resource_processor::trigger_gateway_route_revalidation();

            // 6. Create ConfigSyncServer and register all WatchObjs
            let config_sync_server = Arc::new(ConfigSyncServer::new());
            config_sync_server.set_endpoint_mode(endpoint_mode);
            let no_sync_kinds = crate::core::common::config::get_no_sync_kinds();
            let no_sync_refs: Vec<&str> = no_sync_kinds.iter().map(|s| s.as_str()).collect();
            config_sync_server.register_all(PROCESSOR_REGISTRY.all_watch_objs(&no_sync_refs));

            // 7. Set config_sync_server = Some (services become available)
            self.set_config_sync_server(Some(config_sync_server));

            tracing::info!(
                component = "file_system_center",
                mode = "file_system",
                "ConfigSyncServer is ready, gRPC services can process requests"
            );

            // 8. Wait for shutdown, reload, or error
            tracing::info!(
                component = "file_system_center",
                mode = "file_system",
                "FileSystem mode: running until shutdown or reload signal"
            );

            let mut global_shutdown = shutdown_handle.signal();
            let mut error_rx = error_rx;

            enum LoopAction {
                Shutdown,
                Reload,
                Error(String),
            }

            let action = tokio::select! {
                _ = global_shutdown.wait() => LoopAction::Shutdown,
                _ = reload_rx.recv() => LoopAction::Reload,
                result = &mut error_rx => {
                    match result {
                        Ok(msg) => LoopAction::Error(msg),
                        Err(_) => {
                            // Channel closed without error, wait for shutdown
                            global_shutdown.wait().await;
                            LoopAction::Shutdown
                        }
                    }
                }
            };

            // 9. Stop controller gracefully via iteration_shutdown
            iteration_shutdown.shutdown();

            // 10. Cleanup
            self.set_config_sync_server(None);
            self.abort_controller();
            PROCESSOR_REGISTRY.clear_registry();
            self.set_reload_tx(None);

            // 11. Handle action
            match action {
                LoopAction::Shutdown => {
                    tracing::info!(
                        component = "file_system_center",
                        mode = "file_system",
                        "Normal shutdown, FileSystem lifecycle completed"
                    );
                    return Ok(());
                }
                LoopAction::Reload => {
                    tracing::info!(
                        component = "file_system_center",
                        mode = "file_system",
                        "Reload requested, restarting controller with new server_id"
                    );
                    // Record reload start metrics
                    reload_metrics().reload_started();
                    reload_start_time = Some(Instant::now());
                    // Continue loop to restart
                    continue;
                }
                LoopAction::Error(msg) => {
                    tracing::error!(
                        component = "file_system_center",
                        mode = "file_system",
                        error = %msg,
                        "Controller stopped with error, waiting for shutdown"
                    );
                    // Wait for shutdown signal
                    shutdown_handle.signal().wait().await;
                    tracing::info!(
                        component = "file_system_center",
                        mode = "file_system",
                        "FileSystem lifecycle completed after error"
                    );
                    return Ok(());
                }
            }
        }
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

    /// Request a reload (re-initialize all processors and stores)
    fn request_reload(&self) -> Result<(), String> {
        if let Some(tx) = self.reload_tx.lock().unwrap().as_ref() {
            tx.try_send(())
                .map_err(|e| format!("Failed to send reload signal: {}", e))
        } else {
            Err("Center not started or not ready for reload".to_string())
        }
    }
}

// FileSystemCenter automatically implements ConfCenter via blanket impl
// because it implements both CenterApi and CenterLifeCycle
