//! FileSystem mode lifecycle implementation
//!
//! This module contains the lifecycle management logic for FileSystem mode:
//! - Load resources from local YAML files
//! - Optionally watch for file changes
//! - Handle graceful shutdown

use super::{load_all_resources, ConfCenterConfig, ConfCenter, FileWatcher};
use crate::core::conf_mgr::conf_center::kubernetes::shutdown::ShutdownHandle;
use crate::core::conf_sync::ConfigServer;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::oneshot;

impl ConfCenter {
    /// FileSystem mode lifecycle - simple and direct
    ///
    /// 1. Create ConfigServer
    /// 2. Load resources + start FileWatcher
    /// 3. Set config_server = Some (services become available)
    /// 4. Wait for shutdown signal or watcher error
    pub(super) async fn run_filesystem_lifecycle(&self) -> Result<()> {
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "Starting FileSystem lifecycle"
        );

        // Setup shutdown handling
        let shutdown_handle = ShutdownHandle::new();
        let mut shutdown_signal = shutdown_handle.signal();

        // Spawn signal handler (listens for SIGTERM/SIGINT)
        let signal_handle = shutdown_handle.clone();
        tokio::spawn(async move {
            signal_handle.wait_for_signals().await;
        });

        // Store shutdown handle for external shutdown requests
        {
            let mut handle = self.shutdown_handle.lock().unwrap();
            *handle = Some(shutdown_handle);
        }

        // 1. Create ConfigServer
        let config_server = Arc::new(ConfigServer::new(&self.conf_sync_config));

        // 2. Load resources + start FileWatcher
        let watcher_error_rx = self.start_filesystem_sync(&config_server).await?;

        // 3. Wait for caches to be ready
        self.wait_caches_ready(&config_server, 30).await;

        // 4. Set config_server = Some (services become available)
        self.set_config_server(Some(config_server));
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "ConfigServer is ready, services can process requests"
        );

        // 5. Wait for shutdown signal or watcher error
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "FileSystem mode: running until shutdown signal"
        );

        // Wait for either shutdown signal or watcher error
        if let Some(mut error_rx) = watcher_error_rx {
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
                            "File watcher stopped with error, system continues but won't detect file changes"
                        );
                        // Continue waiting for shutdown - watcher error is not fatal
                        shutdown_signal.wait().await;
                    }
                }
            }
        } else {
            // No watcher running, just wait for shutdown
            shutdown_signal.wait().await;
        }

        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "Cleaning up"
        );

        // 6. Set config_server = None (services become unavailable)
        self.set_config_server(None);

        // 7. Stop watcher if running
        if let Some(handle) = self.watcher_handle.lock().unwrap().take() {
            handle.abort();
        }

        Ok(())
    }

    /// Start FileSystem sync: load resources and optionally start FileWatcher
    ///
    /// Returns an optional receiver for watcher errors.
    /// Note: Assumes shutdown_handle is already set in self.shutdown_handle
    pub(super) async fn start_filesystem_sync(
        &self,
        config_server: &Arc<ConfigServer>,
    ) -> Result<Option<oneshot::Receiver<String>>> {
        let ConfCenterConfig::FileSystem { conf_dir, watch_enabled } = &self.config else {
            return Err(anyhow::anyhow!("Not in FileSystem mode"));
        };

        // Load all resources from file system
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            conf_dir = %conf_dir.display(),
            "Loading all resources from file system"
        );
        load_all_resources(self.writer.clone(), config_server.clone()).await?;

        // Start file watcher if enabled
        if *watch_enabled {
            tracing::info!(
                component = "conf_center",
                mode = "file_system",
                conf_dir = %conf_dir.display(),
                "Starting file watcher"
            );

            // Get shutdown signal from existing handle
            let shutdown_signal = {
                let handle = self.shutdown_handle.lock().unwrap();
                handle.as_ref().map(|h| h.signal())
            };

            let Some(watcher_shutdown_signal) = shutdown_signal else {
                return Err(anyhow::anyhow!("Shutdown handle not set"));
            };

            let watcher_config_server = config_server.clone();
            let watcher = FileWatcher::new(conf_dir.clone(), watcher_config_server);

            // Create error channel
            let (error_tx, error_rx) = oneshot::channel::<String>();

            // Spawn watcher in background
            let handle = tokio::spawn(async move {
                if let Err(e) = watcher.start(watcher_shutdown_signal).await {
                    let error_msg = e.to_string();
                    tracing::error!(
                        component = "conf_center",
                        mode = "file_system",
                        error = %error_msg,
                        "File watcher error"
                    );
                    // Send error to main loop (ignore if receiver dropped)
                    let _ = error_tx.send(error_msg);
                }
            });

            let mut watcher_handle = self.watcher_handle.lock().unwrap();
            *watcher_handle = Some(handle);

            return Ok(Some(error_rx));
        }

        Ok(None)
    }
}
