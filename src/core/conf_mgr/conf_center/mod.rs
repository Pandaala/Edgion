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
pub use file_system::FileSystemWriter;
pub use init_loader::load_all_resources;
pub use kubernetes::{KubernetesController, KubernetesStore, KubernetesWriter};
pub use status::{FileSystemStatusStore, KubernetesStatusStore, StatusStore, StatusStoreError};
pub use traits::{ConfEntry, ConfWriter, ConfWriterError};

use crate::core::conf_sync::ConfigServer;
use anyhow::Result;
use std::sync::Arc;

/// ConfCenter - Unified configuration center
///
/// Provides a unified interface for configuration management regardless of backend.
pub struct ConfCenter {
    config: ConfCenterConfig,
    writer: Arc<dyn ConfWriter>,
    // K8s specific: store reference for controller
    k8s_store: Option<Arc<kubernetes::KubernetesStore>>,
}

impl ConfCenter {
    /// Create a new ConfCenter based on configuration
    pub async fn create(config: ConfCenterConfig) -> Result<Self> {
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
                    k8s_store: None,
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
                    k8s_store: Some(store),
                })
            }
        }
    }

    /// Start the configuration center
    ///
    /// - FileSystem: Load all configs, optionally start file watcher
    /// - Kubernetes: Start controller to watch resources
    pub async fn start(&self, config_server: Arc<ConfigServer>) -> Result<()> {
        match &self.config {
            ConfCenterConfig::FileSystem { watch_enabled, .. } => {
                // Load all resources from file system
                tracing::info!(
                    component = "conf_center",
                    mode = "file_system",
                    "Loading all resources from file system"
                );
                load_all_resources(self.writer.clone(), config_server.clone()).await?;

                // Mark all caches as ready (file system loads synchronously)
                config_server.set_all_ready();

                // Start file watcher if enabled
                if *watch_enabled {
                    tracing::info!(
                        component = "conf_center",
                        mode = "file_system",
                        "Starting file watcher"
                    );
                    // TODO: Implement FileWatcher
                    tracing::warn!(
                        component = "conf_center",
                        "File watcher not yet implemented"
                    );
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
                    config_server,
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

    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        self.config.is_k8s_mode()
    }

    /// Get the configuration
    pub fn config(&self) -> &ConfCenterConfig {
        &self.config
    }
}
