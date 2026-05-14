//! FileSystemController - Backend-specific adapter for FileSystem mode.
//!
//! Wires `FileSystemWatcher` and `FileSystemResourceController` into the shared
//! `LocalControllerBackend` interface so that `spawn_all_controllers` can drive
//! both FileSystem and Etcd modes from a single resource list.
//!
//! ```text
//! FileSystemController::run()
//!     ├── FileSystemControllerBackend (implements LocalControllerBackend)
//!     └── spawn_all_controllers(&backend, ...)
//!             └── per resource: ResourceProcessor → register → subscribe → launch
//! ```

use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::task::JoinHandle;

use super::file_watcher::{FileSystemWatcher, KindEventReceiver};
use super::path_registry::PathRegistry;
use super::resource_controller::FileSystemResourceController;
use super::status::FileSystemStatusHandler;
use crate::core::controller::conf_mgr::conf_center::non_kube_common::{spawn_all_controllers, LocalControllerBackend};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{ResourceProcessor, SecretRefManager};
use crate::core::controller::conf_mgr::sync_runtime::ShutdownSignal;
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::types::ResourceMeta;

/// Backend implementation that wires `FileSystemWatcher` and
/// `FileSystemResourceController` into the shared [`LocalControllerBackend`]
/// interface.
struct FileSystemControllerBackend {
    watcher: Arc<FileSystemWatcher>,
    registry: Arc<PathRegistry>,
}

#[async_trait]
impl LocalControllerBackend for FileSystemControllerBackend {
    type EventReceiver = KindEventReceiver;

    async fn subscribe(&self, kind: &'static str) -> KindEventReceiver {
        self.watcher.subscribe(kind).await
    }

    fn launch<K>(
        &self,
        kind: &'static str,
        processor: Arc<ResourceProcessor<K>>,
        event_rx: KindEventReceiver,
        shutdown_signal: ShutdownSignal,
    ) -> JoinHandle<Result<()>>
    where
        K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    {
        let conf_dir = self.watcher.conf_dir().to_path_buf();
        let ctrl = FileSystemResourceController::new(kind, processor, conf_dir, self.registry.clone(), event_rx)
            .with_shutdown(shutdown_signal);
        tokio::spawn(async move { ctrl.run().await })
    }
}

/// FileSystemController - Top-level controller for FileSystem mode
pub struct FileSystemController {
    conf_dir: PathBuf,
    registry: Arc<PathRegistry>,
}

impl FileSystemController {
    /// Create a new FileSystemController
    pub fn new(conf_dir: PathBuf, registry: Arc<PathRegistry>) -> Self {
        tracing::info!(
            component = "fs_controller",
            conf_dir = %conf_dir.display(),
            "Creating FileSystemController"
        );

        Self { conf_dir, registry }
    }

    /// Run the controller
    pub async fn run(&self, shutdown_signal: ShutdownSignal, runtime: Arc<ControllerRuntime>) -> Result<()> {
        tracing::info!(
            component = "fs_controller",
            conf_dir = %self.conf_dir.display(),
            "Starting FileSystemController"
        );

        // Cleanup orphan .status files at startup. The handler is short-lived but
        // we still feed it the registry so any code path through `build_status_path`
        // (none today, but defense in depth) goes through the canonical lookup.
        let status_handler = FileSystemStatusHandler::new(self.conf_dir.clone()).with_registry(self.registry.clone());
        match status_handler.cleanup_orphans().await {
            Ok(count) if count > 0 => {
                tracing::warn!(
                    component = "fs_controller",
                    cleaned = count,
                    "Cleaned up orphan status files"
                );
            }
            Err(e) => {
                tracing::warn!(
                    component = "fs_controller",
                    error = %e,
                    "Failed to cleanup orphan status files"
                );
            }
            _ => {}
        }

        // Create shared components
        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let watcher = Arc::new(FileSystemWatcher::new(self.conf_dir.clone(), self.registry.clone()));

        let backend = FileSystemControllerBackend {
            watcher: watcher.clone(),
            registry: self.registry.clone(),
        };

        // Spawn all resource controllers
        let handles = spawn_all_controllers(
            &backend,
            &secret_ref_manager,
            shutdown_signal.clone(),
            runtime,
            "fs_controller",
        )
        .await?;

        tracing::info!(
            component = "fs_controller",
            count = handles.len(),
            "All ResourceControllers spawned"
        );

        // Run the watcher (this handles init phase + runtime phase)
        watcher.run(shutdown_signal).await?;

        // Wait for all controllers to finish
        for handle in handles {
            handle.abort();
        }

        tracing::info!(component = "fs_controller", "FileSystemController stopped");

        Ok(())
    }
}
