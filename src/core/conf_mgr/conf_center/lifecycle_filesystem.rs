//! FileSystem mode lifecycle implementation
//!
//! This module contains the lifecycle management logic for FileSystem mode:
//! - Load resources from local YAML files via FileSystemSyncController
//! - Watch for file changes (integrated in sync controller)
//! - Handle graceful shutdown

use super::file_system::FileSystemSyncController;
use super::sync_runtime::ShutdownHandle;
use super::{ConfCenter, ConfCenterConfig, EndpointMode};
use crate::core::conf_sync::ConfigServer;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::oneshot;

impl ConfCenter {
    /// FileSystem mode lifecycle with external shutdown handle
    ///
    /// Uses the new FileSystemSyncController for unified init + runtime flow:
    /// 1. Create ConfigServer
    /// 2. Run FileSystemSyncController (init phase + runtime phase)
    /// 3. Set config_server = Some (services become available)
    /// 4. Wait for shutdown signal
    pub(super) async fn run_filesystem_lifecycle_with_shutdown(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "Starting FileSystem lifecycle"
        );

        let shutdown_signal = shutdown_handle.signal();

        // Store shutdown handle for external shutdown requests
        {
            let mut handle = self.shutdown_handle.lock().unwrap();
            *handle = Some(shutdown_handle);
        }

        // 1. Create ConfigServer with configured endpoint mode
        let config_server = Arc::new(ConfigServer::new(&self.conf_sync_config));
        // Resolve endpoint mode: Auto defaults to EndpointSlice in file system mode
        let endpoint_mode = match self.config.endpoint_mode() {
            EndpointMode::Auto => EndpointMode::EndpointSlice,
            mode => mode,
        };
        config_server.set_endpoint_mode(endpoint_mode);
        crate::core::backends::init_global_endpoint_mode(endpoint_mode);
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            endpoint_mode = ?endpoint_mode,
            "Using endpoint mode"
        );

        // 2. Start FileSystemSyncController (init + runtime)
        let controller_error_rx = self.start_filesystem_sync_controller(&config_server).await?;

        // 3. Wait for caches to be ready (set by sync controller after init phase)
        self.wait_caches_ready(&config_server, 30).await;

        // 4. Set config_server = Some (services become available)
        self.set_config_server(Some(config_server));
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "ConfigServer is ready, services can process requests"
        );

        // 5. Wait for shutdown signal or controller error
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "FileSystem mode: running until shutdown signal"
        );

        let mut shutdown_signal = shutdown_signal;
        if let Some(mut error_rx) = controller_error_rx {
            tokio::select! {
                _ = shutdown_signal.wait() => {
                    tracing::info!(
                        component = "conf_center",
                        mode = "file_system",
                        "Received shutdown signal"
                    );
                }
                result = &mut error_rx => {
                    if let Ok(error_msg) = result {
                        tracing::error!(
                            component = "conf_center",
                            mode = "file_system",
                            error = %error_msg,
                            "Sync controller stopped with error"
                        );
                        // Continue waiting for shutdown - controller error is not fatal
                        shutdown_signal.wait().await;
                    }
                }
            }
        } else {
            shutdown_signal.wait().await;
        }

        tracing::info!(component = "conf_center", mode = "file_system", "Cleaning up");

        // 6. Set config_server = None (services become unavailable)
        self.set_config_server(None);

        // 7. Stop controller if running
        if let Some(handle) = self.watcher_handle.lock().unwrap().take() {
            handle.abort();
        }

        Ok(())
    }

    /// Start FileSystemSyncController
    ///
    /// Creates and runs the sync controller which handles:
    /// - Init phase: scan directory and load all resources
    /// - Runtime phase: watch for file changes and process via workqueue
    async fn start_filesystem_sync_controller(
        &self,
        config_server: &Arc<ConfigServer>,
    ) -> Result<Option<oneshot::Receiver<String>>> {
        let ConfCenterConfig::FileSystem { conf_dir, .. } = &self.config else {
            return Err(anyhow::anyhow!("Not in FileSystem mode"));
        };

        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            conf_dir = %conf_dir.display(),
            "Starting FileSystem sync controller"
        );

        // Get shutdown signal from existing handle
        let shutdown_signal = {
            let handle = self.shutdown_handle.lock().unwrap();
            handle.as_ref().map(|h| h.signal())
        };

        let Some(controller_shutdown_signal) = shutdown_signal else {
            return Err(anyhow::anyhow!("Shutdown handle not set"));
        };

        // Create sync controller
        let controller = FileSystemSyncController::new(conf_dir.clone(), config_server.clone());

        // Create error channel
        let (error_tx, error_rx) = oneshot::channel::<String>();

        // Spawn controller in background
        let handle = tokio::spawn(async move {
            if let Err(e) = controller.run(controller_shutdown_signal).await {
                let error_msg = e.to_string();
                tracing::error!(
                    component = "conf_center",
                    mode = "file_system",
                    error = %error_msg,
                    "Sync controller error"
                );
                let _ = error_tx.send(error_msg);
            }
        });

        // Store handle for cleanup
        let mut watcher_handle = self.watcher_handle.lock().unwrap();
        *watcher_handle = Some(handle);

        Ok(Some(error_rx))
    }
}
