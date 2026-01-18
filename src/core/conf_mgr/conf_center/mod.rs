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
//! ## Lifecycle Management
//!
//! ConfCenter uses a unified start() method that manages the entire lifecycle:
//! - K8s mode: Leader election -> Create ConfigServer -> Link -> Monitor relink/leadership
//! - FileSystem mode: Create ConfigServer -> Link
//!
//! ConfigServer is Option<Arc<ConfigServer>>:
//! - None: Not ready (during startup, relink, or leadership loss)
//! - Some: Ready to serve requests
//!
//! gRPC and Admin services get ConfigServer dynamically via config_server() method.
//! When ConfigServer is None, they return UNAVAILABLE/NOT_READY errors.

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
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// ConfCenter - Unified configuration center
///
/// Provides a unified interface for configuration management regardless of backend.
/// Manages ConfigServer lifecycle internally - ConfigServer is Option<Arc<ConfigServer>>:
/// - None: System not ready (during startup, relink, or leadership loss)
/// - Some: System ready to serve requests
///
/// ## Lifecycle
///
/// The start() method manages the entire lifecycle:
/// 1. K8s mode: Wait for leadership -> Create ConfigServer -> Link -> Monitor
/// 2. FileSystem mode: Create ConfigServer -> Link
///
/// gRPC and Admin services get ConfigServer via config_server() method.
/// When None, they should return UNAVAILABLE errors.
pub struct ConfCenter {
    config: ConfCenterConfig,
    conf_sync_config: ConfSyncConfig,
    writer: Arc<dyn ConfWriter>,
    
    /// ConfigServer instance - Option to support lifecycle management
    /// None: Not ready (startup, relink, leadership loss)
    /// Some: Ready to serve requests
    config_server: RwLock<Option<Arc<ConfigServer>>>,
    
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
    /// Note: ConfigServer is NOT created here. It will be created in start() method
    /// after successful leader election (K8s mode) or immediately (FileSystem mode).
    pub async fn create(
        config: ConfCenterConfig,
        conf_sync_config: &ConfSyncConfig,
    ) -> Result<Self> {
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
                    config_server: RwLock::new(None),
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
                    config_server: RwLock::new(None),
                    shutdown_handle: Mutex::new(None),
                    controller_handle: Mutex::new(None),
                    exit_reason_rx: Mutex::new(None),
                })
            }
        }
    }

    /// Unified start method - manages the entire lifecycle
    ///
    /// This method runs a loop that:
    /// 1. K8s mode: Wait for leadership first (TODO: implement leader election)
    /// 2. Create new ConfigServer
    /// 3. Link (start controller/watcher, load resources)
    /// 4. Wait for caches to be ready
    /// 5. Set config_server = Some (services become available)
    /// 6. Wait for exit signal (relink needed, leadership lost, etc.)
    /// 7. Set config_server = None (services become unavailable)
    /// 8. Loop back based on exit reason
    ///
    /// For FileSystem mode, this runs once and blocks on the watcher.
    pub async fn start(&self) -> Result<()> {
        const MAX_RELINK_RETRIES: u32 = 10;
        const RELINK_BACKOFF_BASE_SECS: u64 = 1;
        const RELINK_BACKOFF_MAX_SECS: u64 = 60;
        const CACHE_READY_TIMEOUT_SECS: u64 = 30;

        let mut relink_count: u32 = 0;

        loop {
            tracing::info!(
                component = "conf_center",
                mode = if self.is_k8s_mode() { "kubernetes" } else { "file_system" },
                relink_count = relink_count,
                "ConfCenter start: beginning lifecycle iteration"
            );

            // TODO: K8s mode - wait for leadership here
            // if self.is_k8s_mode() {
            //     self.wait_for_leadership().await?;
            // }

            // 1. Create new ConfigServer
            let config_server = Arc::new(ConfigServer::new(&self.conf_sync_config));
            
            // 2. Link with the new ConfigServer
            self.link_with_server(&config_server).await?;

            // 3. Wait for caches to be ready
            self.wait_caches_ready(&config_server, CACHE_READY_TIMEOUT_SECS).await;

            // 4. Set config_server = Some (services become available)
            self.set_config_server(Some(config_server.clone()));
            tracing::info!(
                component = "conf_center",
                event = "config_server_ready",
                "ConfigServer is now ready, services can process requests"
            );

            // 5. Wait for exit signal
            let exit_reason = self.wait_for_exit().await;

            match exit_reason {
                Some(reason) => {
                    tracing::info!(
                        component = "conf_center",
                        event = "exit_signal",
                        reason = ?reason,
                        relink_count = relink_count,
                        "Received exit signal"
                    );

                    // 6. Set config_server = None (services become unavailable)
                    self.set_config_server(None);

                    if Self::needs_relink(&reason) {
                        relink_count += 1;

                        if relink_count > MAX_RELINK_RETRIES {
                            tracing::error!(
                                component = "conf_center",
                                event = "max_relink_retries",
                                relink_count = relink_count,
                                "Maximum relink retries exceeded, stopping"
                            );
                            return Err(anyhow::anyhow!("Maximum relink retries exceeded"));
                        }

                        // Calculate backoff delay
                        let backoff_secs = std::cmp::min(
                            RELINK_BACKOFF_BASE_SECS * 2u64.pow(relink_count.saturating_sub(1)),
                            RELINK_BACKOFF_MAX_SECS,
                        );

                        tracing::info!(
                            component = "conf_center",
                            event = "relink_scheduled",
                            relink_count = relink_count,
                            backoff_secs = backoff_secs,
                            "Scheduling relink with backoff"
                        );

                        // Cleanup current state
                        self.cleanup().await;

                        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;

                        // Reset relink_count on successful iteration
                        // (will be set back to 0 after successful link)
                    } else {
                        // Normal exit - cleanup and return
                        tracing::info!(
                            component = "conf_center",
                            event = "normal_exit",
                            reason = ?reason,
                            "Normal exit, stopping ConfCenter"
                        );
                        self.cleanup().await;
                        return Ok(());
                    }
                }
                None => {
                    // FileSystem mode or no controller - just wait indefinitely
                    if !self.is_k8s_mode() {
                        tracing::info!(
                            component = "conf_center",
                            mode = "file_system",
                            "FileSystem mode: blocking until shutdown"
                        );
                        // Block forever (or until shutdown signal)
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                        }
                    }
                    // K8s mode but no exit reason - shouldn't happen, break
                    tracing::warn!(
                        component = "conf_center",
                        "No exit reason received, stopping"
                    );
                    self.cleanup().await;
                    return Ok(());
                }
            }

            // Reset relink count after successful relink
            relink_count = 0;
        }
    }

    /// Link with a specific ConfigServer instance
    ///
    /// This method:
    /// 1. Creates shutdown handle
    /// 2. Starts the appropriate sync mechanism (FileSystem watcher or K8s controller)
    async fn link_with_server(&self, config_server: &Arc<ConfigServer>) -> Result<()> {
        tracing::info!(
            component = "conf_center",
            mode = if self.is_k8s_mode() { "kubernetes" } else { "file_system" },
            "ConfCenter link: starting configuration sync"
        );

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
                
                load_all_resources(self.writer.clone(), config_server.clone()).await?;

                // Start file watcher if enabled
                if *watch_enabled {
                    tracing::info!(
                        component = "conf_center",
                        mode = "file_system",
                        conf_dir = %conf_dir.display(),
                        "Starting file watcher"
                    );

                    let watcher_config_server = config_server.clone();
                    let watcher = FileWatcher::new(conf_dir.clone(), watcher_config_server);
                    let watcher_shutdown_signal = shutdown_handle.signal();

                    // Spawn watcher in background with shutdown support
                    let handle = tokio::spawn(async move {
                        if let Err(e) = watcher.start(watcher_shutdown_signal).await {
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
                    config_server.clone(),
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

    /// Wait for all caches to be ready
    async fn wait_caches_ready(&self, config_server: &Arc<ConfigServer>, timeout_secs: u64) {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);

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
            tracing::info!(
                component = "conf_center",
                event = "waiting_for_caches",
                not_ready = ?not_ready,
                elapsed_ms = start.elapsed().as_millis(),
                "Waiting for caches to be ready..."
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    /// Set the ConfigServer (Some = ready, None = not ready)
    fn set_config_server(&self, server: Option<Arc<ConfigServer>>) {
        let mut config_server = self.config_server.write().unwrap();
        let was_some = config_server.is_some();
        let is_some = server.is_some();
        *config_server = server;
        
        tracing::info!(
            component = "conf_center",
            event = "config_server_changed",
            was_ready = was_some,
            is_ready = is_some,
            "ConfigServer state changed"
        );
    }

    /// Cleanup: Stop sync tasks and clear state
    async fn cleanup(&self) {
        tracing::info!(
            component = "conf_center",
            mode = if self.is_k8s_mode() { "kubernetes" } else { "file_system" },
            "ConfCenter cleanup: stopping sync tasks"
        );

        // 1. Trigger shutdown signal
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

        // 2. Wait for controller task to finish (with timeout)
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

        // 3. Clear shutdown handle
        {
            let mut handle = self.shutdown_handle.lock().unwrap();
            *handle = None;
        }

        // 4. Clear exit reason receiver
        {
            let mut rx = self.exit_reason_rx.lock().unwrap();
            *rx = None;
        }

        tracing::info!(
            component = "conf_center",
            "ConfCenter cleanup complete"
        );
    }

    /// Wait for controller to exit and return the exit reason
    /// 
    /// This method is used by the lifecycle loop to detect when relink is needed.
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
            Some(ref mut receiver) => receiver.recv().await,
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
}
