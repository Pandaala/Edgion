//! FileSystem mode lifecycle implementation
//!
//! This module contains the lifecycle management logic for FileSystem mode:
//! - Run FileSystemController to load and watch resources
//! - Wait for PROCESSOR_REGISTRY to be ready
//! - Create ConfigSyncServer and register WatchObjs
//! - Handle graceful shutdown

use super::conf_center::ConfCenter;
use super::config::{ConfCenterConfig, EndpointMode};
use super::file_system::FileSystemController;
use crate::core::conf_mgr_new::sync_runtime::ShutdownHandle;
use crate::core::conf_mgr_new::PROCESSOR_REGISTRY;
use crate::core::conf_sync::conf_server_new::ConfigSyncServer;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::oneshot;

impl ConfCenter {
    /// FileSystem mode lifecycle with external shutdown handle
    ///
    /// Uses the new FileSystemController for unified init + runtime flow:
    /// 1. Resolve endpoint mode
    /// 2. Run FileSystemController (registers to PROCESSOR_REGISTRY, runs init + runtime)
    /// 3. Wait for PROCESSOR_REGISTRY to be ready
    /// 4. Create ConfigSyncServer and register WatchObjs
    /// 5. Set config_sync_server = Some (services become available)
    /// 6. Wait for shutdown signal
    pub(super) async fn run_filesystem_lifecycle_with_shutdown(
        &self,
        shutdown_handle: ShutdownHandle,
    ) -> Result<()> {
        tracing::info!(
            component = "conf_center_new",
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
            component = "conf_center_new",
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
                    component = "conf_center_new",
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
            component = "conf_center_new",
            mode = "file_system",
            "ConfigSyncServer is ready, gRPC services can process requests"
        );

        // 8. Wait for shutdown signal or controller error
        tracing::info!(
            component = "conf_center_new",
            mode = "file_system",
            "FileSystem mode: running until shutdown signal"
        );

        let mut shutdown_signal = shutdown_signal;
        let mut error_rx = error_rx;

        tokio::select! {
            _ = shutdown_signal.wait() => {
                tracing::info!(
                    component = "conf_center_new",
                    mode = "file_system",
                    "Received shutdown signal"
                );
            }
            result = &mut error_rx => {
                if let Ok(error_msg) = result {
                    tracing::error!(
                        component = "conf_center_new",
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
            component = "conf_center_new",
            mode = "file_system",
            "Cleaning up"
        );

        // 9. Cleanup
        self.set_config_sync_server(None);
        self.abort_controller();

        // 10. Clear PROCESSOR_REGISTRY
        PROCESSOR_REGISTRY.clear_registry();

        tracing::info!(
            component = "conf_center_new",
            mode = "file_system",
            "FileSystem lifecycle completed"
        );

        Ok(())
    }
}
