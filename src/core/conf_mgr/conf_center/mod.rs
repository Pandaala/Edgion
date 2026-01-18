//! Configuration Center (ConfCenter)
//!
//! Unified configuration management supporting multiple backends:
//! - FileSystem: Local YAML files with optional file watching
//! - Kubernetes: K8s API with kube-runtime Controller-based resource watching
//!
//! Architecture:
//! ```text
//! ConfCenter
//! ├── FileSystem Mode
//! │   ├── FileSystemWriter (ConfWriter impl) - read/write local files
//! │   └── FileWatcher - watch file changes, notify ConfigServer
//! └── Kubernetes Mode
//!     ├── KubernetesWriter (ConfWriter impl) - call K8s API
//!     ├── KubernetesController - kube-runtime Controller pattern
//!     └── ResourceStores - reflector::Store for each resource type
//! ```
//!
//! ## Lifecycle Management (link/unlink/relink)
//!
//! ConfCenter provides lifecycle methods to handle:
//! - 410 Gone (resourceVersion expired, needs re-LIST)
//! - Leader re-election (lost/regained leadership)
//! - Any scenario requiring full state reset
//!
//! ```text
//! link() -> running -> unlink() -> link() -> ...
//!              │
//!              └── relink() = unlink() + link()
//! ```

mod config;
pub mod file_system;
pub mod init_loader;
pub mod kubernetes;
pub mod status;
pub mod traits;

pub use config::ConfCenterConfig;
pub use file_system::{FileSystemWriter, FileWatcher};
pub use init_loader::load_all_resources;
pub use kubernetes::{ControllerExitReason, KubernetesController, KubernetesStatusStore, KubernetesWriter, NamespaceWatchMode, RelinkReason, StatusStore, StatusStoreError};
pub use status::FileSystemStatusStore;
pub use traits::{ConfEntry, ConfWriter, ConfWriterError, ListOptions, ListResult};

use crate::core::cli::config::ConfSyncConfig;
use crate::core::conf_sync::ConfigServer;
use anyhow::Result;
use kubernetes::shutdown::ShutdownHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// ConfCenter - Unified configuration center
///
/// Provides a unified interface for configuration management regardless of backend.
/// Internally holds the ConfigServer instance and global ready state.
///
/// ## Lifecycle
///
/// ConfCenter supports link/unlink/relink lifecycle for handling:
/// - 410 Gone errors (resourceVersion expired)
/// - Leader re-election scenarios
/// - Any situation requiring full state reset
pub struct ConfCenter {
    config: ConfCenterConfig,
    #[allow(dead_code)]
    conf_sync_config: ConfSyncConfig,
    writer: Arc<dyn ConfWriter>,
    config_server: Arc<ConfigServer>,
    /// Global all_ready flag - controlled at Controller level
    /// When true, ConfigServer and Admin API can serve requests
    all_ready: Arc<AtomicBool>,
    /// Shutdown handle for stopping sync tasks
    shutdown_handle: Mutex<Option<ShutdownHandle>>,
    /// Handle to the running controller task (K8s mode)
    controller_handle: Mutex<Option<JoinHandle<()>>>,
    /// Channel to receive controller exit reason (K8s mode)
    exit_reason_rx: Mutex<Option<mpsc::Receiver<ControllerExitReason>>>,
}

impl ConfCenter {
    /// Create a new ConfCenter based on configuration
    ///
    /// This creates the ConfigServer internally based on the provided ConfSyncConfig.
    /// The all_ready flag is passed from Controller level for global state management.
    pub async fn create(
        config: ConfCenterConfig,
        conf_sync_config: &ConfSyncConfig,
        all_ready: Arc<AtomicBool>,
    ) -> Result<Self> {
        // Create ConfigServer internally, passing the shared all_ready flag
        let config_server = Arc::new(ConfigServer::new(conf_sync_config, all_ready.clone()));

        match &config {
            ConfCenterConfig::FileSystem { conf_dir, .. } => {
                tracing::info!(
                    component = "conf_center",
                    mode = "file_system",
                    conf_dir = %conf_dir.display(),
                    "Creating FileSystem ConfCenter"
                );
                let writer = FileSystemWriter::new(conf_dir);
                Ok(Self {
                    config,
                    conf_sync_config: conf_sync_config.clone(),
                    writer: Arc::new(writer),
                    config_server,
                    all_ready,
                    shutdown_handle: Mutex::new(None),
                    controller_handle: Mutex::new(None),
                    exit_reason_rx: Mutex::new(None),
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
                    config,
                    conf_sync_config: conf_sync_config.clone(),
                    writer: Arc::new(writer),
                    config_server,
                    all_ready,
                    shutdown_handle: Mutex::new(None),
                    controller_handle: Mutex::new(None),
                    exit_reason_rx: Mutex::new(None),
                })
            }
        }
    }

    /// Start the configuration center (calls link internally)
    ///
    /// - FileSystem: Load all configs, optionally start file watcher
    /// - Kubernetes: Start controller to watch resources
    pub async fn start(&self) -> Result<()> {
        self.link().await
    }

    /// Link: Start configuration sync
    ///
    /// This method:
    /// 1. Clears old caches and regenerates server ID (if not first link)
    /// 2. Starts the appropriate sync mechanism (FileSystem watcher or K8s controller)
    /// 3. Waits for all caches to become ready (via external monitoring)
    pub async fn link(&self) -> Result<()> {
        tracing::info!(
            component = "conf_center",
            mode = if self.is_k8s_mode() { "kubernetes" } else { "file_system" },
            "ConfCenter link: starting configuration sync"
        );

        // If we already have a shutdown handle, this is a relink - reset ConfigServer first
        {
            let existing = self.shutdown_handle.lock().unwrap();
            if existing.is_some() {
                // This is a relink, reset ConfigServer
                self.config_server.reset_for_relink();
            }
        }

        // Create new shutdown handle for this link cycle
        let shutdown_handle = ShutdownHandle::new();
        {
            let mut handle = self.shutdown_handle.lock().unwrap();
            *handle = Some(shutdown_handle.clone());
        }

        match &self.config {
            ConfCenterConfig::FileSystem {
                conf_dir,
                watch_enabled,
            } => {
                // Load all resources from file system
                tracing::info!(
                    component = "conf_center",
                    mode = "file_system",
                    "Loading all resources from file system"
                );
                
                load_all_resources(self.writer.clone(), self.config_server.clone()).await?;

                // Note: set_all_ready() will be called by init_loader via InitDone events

                // Start file watcher if enabled
                if *watch_enabled {
                    tracing::info!(
                        component = "conf_center",
                        mode = "file_system",
                        conf_dir = %conf_dir.display(),
                        "Starting file watcher"
                    );

                    let config_server = self.config_server.clone();
                    let watcher = FileWatcher::new(conf_dir.clone(), config_server);

                    // Spawn watcher in background
                    let handle = tokio::spawn(async move {
                        if let Err(e) = watcher.start().await {
                            tracing::error!(
                                component = "conf_center",
                                mode = "file_system",
                                error = %e,
                                "File watcher error"
                            );
                        }
                    });

                    let mut controller_handle = self.controller_handle.lock().unwrap();
                    *controller_handle = Some(handle);
                }

                Ok(())
            }
            ConfCenterConfig::Kubernetes {
                watch_namespaces,
                label_selector,
                gateway_class,
            } => {
                tracing::info!(
                    component = "conf_center",
                    mode = "kubernetes",
                    gateway_class = gateway_class,
                    namespaces = ?watch_namespaces,
                    "Starting Kubernetes controller"
                );

                let controller = KubernetesController::new(
                    self.config_server.clone(),
                    gateway_class.clone(),
                    watch_namespaces.clone(),
                    label_selector.clone(),
                )
                .await?;

                // Create channel for receiving exit reason
                let (exit_tx, exit_rx) = mpsc::channel::<ControllerExitReason>(1);
                {
                    let mut rx = self.exit_reason_rx.lock().unwrap();
                    *rx = Some(exit_rx);
                }

                // Spawn controller in background
                let handle = tokio::spawn(async move {
                    let exit_reason = match controller.run().await {
                        Ok(reason) => {
                            tracing::warn!(
                                component = "conf_center",
                                mode = "kubernetes",
                                exit_reason = ?reason,
                                "Kubernetes controller exited"
                            );
                            reason
                        }
                        Err(e) => {
                            tracing::error!(
                                component = "conf_center",
                                mode = "kubernetes",
                                error = %e,
                                "Kubernetes controller error"
                            );
                            ControllerExitReason::AllControllersStopped
                        }
                    };
                    // Send exit reason to supervisor
                    let _ = exit_tx.send(exit_reason).await;
                });

                let mut controller_handle = self.controller_handle.lock().unwrap();
                *controller_handle = Some(handle);

                Ok(())
            }
        }
    }

    /// Unlink: Stop configuration sync and clear state
    ///
    /// This method:
    /// 1. Sets all_ready to false
    /// 2. Triggers shutdown signal to stop sync tasks
    /// 3. Clears all caches
    pub async fn unlink(&self) -> Result<()> {
        tracing::info!(
            component = "conf_center",
            mode = if self.is_k8s_mode() { "kubernetes" } else { "file_system" },
            "ConfCenter unlink: stopping configuration sync"
        );

        // 1. Set all_ready to false
        self.all_ready.store(false, Ordering::SeqCst);
        tracing::info!(
            component = "conf_center",
            "Set all_ready to false"
        );

        // 2. Trigger shutdown signal
        {
            let handle = self.shutdown_handle.lock().unwrap();
            if let Some(ref shutdown) = *handle {
                shutdown.shutdown();
                tracing::info!(
                    component = "conf_center",
                    "Triggered shutdown signal"
                );
            }
        }

        // 3. Wait for controller task to finish (with timeout)
        let controller_handle = {
            let mut handle = self.controller_handle.lock().unwrap();
            handle.take()
        };
        
        if let Some(handle) = controller_handle {
            match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                Ok(_) => {
                    tracing::info!(
                        component = "conf_center",
                        "Controller task stopped"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        component = "conf_center",
                        "Controller task did not stop within timeout, continuing"
                    );
                }
            }
        }

        // 4. Clear shutdown handle
        {
            let mut handle = self.shutdown_handle.lock().unwrap();
            *handle = None;
        }

        tracing::info!(
            component = "conf_center",
            "ConfCenter unlink complete"
        );

        Ok(())
    }

    /// Relink: Stop and restart configuration sync
    ///
    /// This is equivalent to calling unlink() followed by link().
    /// Used for handling 410 Gone or leader re-election.
    pub async fn relink(&self) -> Result<()> {
        tracing::info!(
            component = "conf_center",
            "ConfCenter relink: stopping and restarting configuration sync"
        );

        self.unlink().await?;
        self.link().await
    }

    /// Wait for controller to exit and return the exit reason
    /// 
    /// This method is used by the supervisor loop to detect when relink is needed.
    /// Only applicable for Kubernetes mode.
    /// 
    /// Returns:
    /// - Some(ControllerExitReason) - Controller exited with a reason
    /// - None - Not in Kubernetes mode or no controller running
    pub async fn wait_for_exit(&self) -> Option<ControllerExitReason> {
        if !self.is_k8s_mode() {
            return None;
        }

        let mut rx = {
            let mut rx_guard = self.exit_reason_rx.lock().unwrap();
            rx_guard.take()
        };

        match rx {
            Some(ref mut receiver) => {
                let reason = receiver.recv().await;
                // Put the receiver back for potential future use
                {
                    let mut rx_guard = self.exit_reason_rx.lock().unwrap();
                    *rx_guard = rx;
                }
                reason
            }
            None => None,
        }
    }

    /// Check if relink is needed based on exit reason
    pub fn needs_relink(reason: &ControllerExitReason) -> bool {
        matches!(
            reason,
            ControllerExitReason::RelinkRequested(_) | ControllerExitReason::LostLeadership
        )
    }

    /// Get the configuration writer
    pub fn writer(&self) -> Arc<dyn ConfWriter> {
        self.writer.clone()
    }

    /// Get the ConfigServer
    pub fn config_server(&self) -> Arc<ConfigServer> {
        self.config_server.clone()
    }

    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        self.config.is_k8s_mode()
    }

    /// Get the configuration
    pub fn config(&self) -> &ConfCenterConfig {
        &self.config
    }

    /// Check if the system is ready (all caches loaded)
    pub fn is_all_ready(&self) -> bool {
        self.all_ready.load(Ordering::SeqCst)
    }

    /// Set the system ready state
    /// Called by Controller after all caches are verified ready
    pub fn set_all_ready(&self) {
        self.all_ready.store(true, Ordering::SeqCst);
        tracing::info!(
            component = "conf_center",
            event = "all_ready",
            "System all_ready state set to true"
        );
    }
}
