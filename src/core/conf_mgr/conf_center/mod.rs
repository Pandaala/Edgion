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
//!     ├── KubernetesController - kube-runtime Controller pattern (includes leader election)
//!     └── ResourceStores - reflector::Store for each resource type
//! ```
//!
//! ## Lifecycle Management
//!
//! ConfCenter uses `start()` which dispatches to mode-specific lifecycle methods:
//!
//! - **FileSystem mode**: Simple and direct
//!   1. Create ConfigServer
//!   2. Load resources + start FileWatcher
//!   3. Block until shutdown
//!
//! - **K8s mode**: Clean loop with retry
//!   1. Create ConfigServer
//!   2. Create and run Controller (includes leader election)
//!   3. On exit, restart with backoff if needed
//!
//! ConfigServer is `Option<Arc<ConfigServer>>`:
//! - None: Not ready (startup, restart, leadership loss)
//! - Some: Ready to serve requests
//!
//! gRPC and Admin services get ConfigServer via `config_server()` method.
//! When None, they return UNAVAILABLE errors.

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
/// - FileSystem: Simple one-shot setup, then block
/// - K8s: Loop with automatic restart on failure
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
    
    /// Shutdown handle for stopping sync tasks (FileSystem mode)
    shutdown_handle: Mutex<Option<ShutdownHandle>>,
    /// Handle to the running watcher task (FileSystem mode)
    watcher_handle: Mutex<Option<JoinHandle<()>>>,
}

impl ConfCenter {
    /// Create a new ConfCenter based on configuration
    ///
    /// Note: ConfigServer is NOT created here. It will be created in `start()` method.
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
                    config,
                    conf_sync_config: conf_sync_config.clone(),
                    writer: Arc::new(writer),
                    config_server: RwLock::new(None),
                    shutdown_handle: Mutex::new(None),
                    watcher_handle: Mutex::new(None),
                })
            }
        }
    }

    // ==================== Lifecycle Management ====================

    /// Start the configuration center
    ///
    /// Dispatches to mode-specific lifecycle methods:
    /// - FileSystem: Simple one-shot setup, then block
    /// - K8s: Loop with automatic restart on failure
    pub async fn start(&self) -> Result<()> {
        if self.is_k8s_mode() {
            self.run_k8s_lifecycle().await
        } else {
            self.run_filesystem_lifecycle().await
        }
    }

    // ==================== FileSystem Mode ====================

    /// FileSystem mode lifecycle - simple and direct
    ///
    /// 1. Create ConfigServer
    /// 2. Load resources + start FileWatcher
    /// 3. Set config_server = Some (services become available)
    /// 4. Block until shutdown
    async fn run_filesystem_lifecycle(&self) -> Result<()> {
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "Starting FileSystem lifecycle"
        );

        // 1. Create ConfigServer
        let config_server = Arc::new(ConfigServer::new(&self.conf_sync_config));

        // 2. Load resources + start FileWatcher
        self.start_filesystem_sync(&config_server).await?;

        // 3. Wait for caches to be ready
        self.wait_caches_ready(&config_server, 30).await;

        // 4. Set config_server = Some (services become available)
        self.set_config_server(Some(config_server));
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "ConfigServer is ready, services can process requests"
        );

        // 5. Block until shutdown
        tracing::info!(
            component = "conf_center",
            mode = "file_system",
            "FileSystem mode: running until shutdown"
        );
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }

    /// Start FileSystem sync: load resources and optionally start FileWatcher
    async fn start_filesystem_sync(&self, config_server: &Arc<ConfigServer>) -> Result<()> {
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

            let shutdown_handle = ShutdownHandle::new();
            {
                let mut handle = self.shutdown_handle.lock().unwrap();
                *handle = Some(shutdown_handle.clone());
            }

            let watcher_config_server = config_server.clone();
            let watcher = FileWatcher::new(conf_dir.clone(), watcher_config_server);
            let watcher_shutdown_signal = shutdown_handle.signal();

            // Spawn watcher in background
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

            let mut watcher_handle = self.watcher_handle.lock().unwrap();
            *watcher_handle = Some(handle);
        }

        Ok(())
    }

    // ==================== Kubernetes Mode ====================

    /// K8s mode lifecycle - clean loop with automatic restart
    ///
    /// Loop:
    /// 1. Create ConfigServer
    /// 2. Create and run Controller (includes leader election)
    /// 3. Wait for caches ready, then set config_server = Some
    /// 4. Wait for controller to exit
    /// 5. Set config_server = None
    /// 6. Handle exit reason: shutdown or restart with backoff
    async fn run_k8s_lifecycle(&self) -> Result<()> {
        const MAX_CONSECUTIVE_FAILURES: u32 = 10;
        const STABLE_RUN_DURATION: Duration = Duration::from_secs(300); // 5 minutes

        let mut consecutive_failures: u32 = 0;

        loop {
            tracing::info!(
                component = "conf_center",
                mode = "kubernetes",
                consecutive_failures = consecutive_failures,
                "Starting K8s lifecycle iteration"
            );

            let iteration_start = Instant::now();

            // 1. Create ConfigServer
            let config_server = Arc::new(ConfigServer::new(&self.conf_sync_config));

            // 2. Create Controller (includes leader election internally)
            let controller = match self.create_k8s_controller(&config_server).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(
                        component = "conf_center",
                        mode = "kubernetes",
                        error = %e,
                        "Failed to create K8s controller"
                    );
                    consecutive_failures += 1;
                    if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                        return Err(anyhow::anyhow!("Max consecutive failures exceeded: {}", e));
                    }
                    let backoff = Duration::from_secs(1 << consecutive_failures.min(6));
                    tokio::time::sleep(backoff).await;
                    continue;
                }
            };

            // 3. Wait for caches to be ready
            self.wait_caches_ready(&config_server, 30).await;

            // 4. Set config_server = Some (services become available)
            self.set_config_server(Some(config_server));
            tracing::info!(
                component = "conf_center",
                mode = "kubernetes",
                "ConfigServer is ready, services can process requests"
            );

            // 5. Run controller (blocks until exit)
            let exit_reason = match controller.run().await {
                Ok(reason) => reason,
                Err(e) => {
                    tracing::error!(
                        component = "conf_center",
                        mode = "kubernetes",
                        error = %e,
                        "Controller run error"
                    );
                    ControllerExitReason::AllControllersStopped
                }
            };

            // 6. Set config_server = None (services become unavailable)
            self.set_config_server(None);

            // 7. Handle exit reason
            match exit_reason {
                ControllerExitReason::Shutdown => {
                    tracing::info!(
                        component = "conf_center",
                        mode = "kubernetes",
                        "Normal shutdown"
                    );
                    return Ok(());
                }
                reason => {
                    tracing::warn!(
                        component = "conf_center",
                        mode = "kubernetes",
                        exit_reason = ?reason,
                        "Controller exited, will restart"
                    );

                    // Reset counter if ran stably for long enough
                    if iteration_start.elapsed() >= STABLE_RUN_DURATION {
                        consecutive_failures = 0;
                    }

                    consecutive_failures += 1;
                    if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                        return Err(anyhow::anyhow!(
                            "Max consecutive failures exceeded after {:?}",
                            reason
                        ));
                    }

                    // Backoff before restart
                    let backoff = Duration::from_secs(1 << consecutive_failures.min(6));
                    tracing::info!(
                        component = "conf_center",
                        mode = "kubernetes",
                        backoff_secs = backoff.as_secs(),
                        consecutive_failures = consecutive_failures,
                        "Waiting before restart"
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    /// Create K8s controller
    async fn create_k8s_controller(&self, config_server: &Arc<ConfigServer>) -> Result<KubernetesController> {
        let ConfCenterConfig::Kubernetes {
            watch_namespaces,
            label_selector,
            gateway_class,
        } = &self.config else {
            return Err(anyhow::anyhow!("Not in Kubernetes mode"));
        };

        tracing::info!(
            component = "conf_center",
            mode = "kubernetes",
            gateway_class = gateway_class,
            namespaces = ?watch_namespaces,
            "Creating Kubernetes controller"
        );

        KubernetesController::new(
            config_server.clone(),
            gateway_class.clone(),
            watch_namespaces.clone(),
            label_selector.clone(),
        )
        .await
    }

    // ==================== Helper Methods ====================

    /// Wait for all caches to be ready
    async fn wait_caches_ready(&self, config_server: &Arc<ConfigServer>, timeout_secs: u64) {
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
    fn set_config_server(&self, server: Option<Arc<ConfigServer>>) {
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
}
