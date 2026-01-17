//! Configuration Center (ConfCenter)
//!
//! Unified configuration management supporting multiple backends:
//! - FileSystem: Local YAML files with optional file watching
//! - Kubernetes: K8s API with Controller-based resource watching
//!
//! Architecture:
//! ```text
//! ConfCenter
//! ├── FileSystem Mode
//! │   ├── FileSystemWriter (ConfWriter impl) - read/write local files
//! │   └── FileWatcher - watch file changes, notify ConfigServer
//! └── Kubernetes Mode
//!     ├── KubernetesWriter (ConfWriter impl) - call K8s API
//!     └── Controller - watch K8s resources, notify ConfigServer
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
pub use kubernetes::{KubernetesController, KubernetesStore, KubernetesWriter};
pub use status::{FileSystemStatusStore, KubernetesStatusStore, StatusStore, StatusStoreError};
pub use traits::{ConfEntry, ConfWriter, ConfWriterError};

use crate::core::cli::config::ConfSyncConfig;
use crate::core::conf_sync::ConfigServer;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// ConfCenter - Unified configuration center
///
/// Provides a unified interface for configuration management regardless of backend.
/// Internally holds the ConfigServer instance and global ready state.
pub struct ConfCenter {
    config: ConfCenterConfig,
    writer: Arc<dyn ConfWriter>,
    config_server: Arc<ConfigServer>,
    // K8s specific: store reference for controller
    k8s_store: Option<Arc<kubernetes::KubernetesStore>>,
    /// Global all_ready flag - controlled at Controller level
    /// When true, ConfigServer and Admin API can serve requests
    all_ready: Arc<AtomicBool>,
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
                    writer: Arc::new(writer),
                    config_server,
                    k8s_store: None,
                    all_ready,
                })
            }
            ConfCenterConfig::Kubernetes { .. } => {
                tracing::info!(
                    component = "conf_center",
                    mode = "kubernetes",
                    "Creating Kubernetes ConfCenter"
                );
                let (writer, store) = KubernetesWriter::new().await?;
                Ok(Self {
                    config,
                    writer: Arc::new(writer),
                    config_server,
                    k8s_store: Some(store),
                    all_ready,
                })
            }
        }
    }

    /// Start the configuration center
    ///
    /// - FileSystem: Load all configs, optionally start file watcher
    /// - Kubernetes: Start controller to watch resources
    pub async fn start(&self) -> Result<()> {
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

                    let watcher = FileWatcher::new(conf_dir.clone(), self.config_server.clone());

                    // Spawn watcher in background
                    tokio::spawn(async move {
                        if let Err(e) = watcher.start().await {
                            tracing::error!(
                                component = "conf_center",
                                mode = "file_system",
                                error = %e,
                                "File watcher error"
                            );
                        }
                    });
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

                let store = self
                    .k8s_store
                    .as_ref()
                    .expect("K8s store should be set in Kubernetes mode");

                let controller = KubernetesController::new(
                    self.config_server.clone(),
                    store.clone(),
                    gateway_class.clone(),
                    watch_namespaces.clone(),
                    label_selector.clone(),
                )
                .await?;

                // Spawn controller in background
                tokio::spawn(async move {
                    if let Err(e) = controller.run().await {
                        tracing::error!(
                            component = "conf_center",
                            mode = "kubernetes",
                            error = %e,
                            "Kubernetes controller error"
                        );
                    }
                });

                Ok(())
            }
        }
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
