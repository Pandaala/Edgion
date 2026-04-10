//! FileSystemController - Top-level controller for FileSystem mode
//!
//! Spawns independent ResourceControllers for each resource type,
//! similar to how KubernetesController works.
//!
//! Architecture:
//! ```text
//! FileSystemController.run()
//!     │
//!     ├── Create FileSystemWatcher (shared)
//!     │
//!     ├── spawn::<HTTPRoute, _>(HttpRouteHandler)
//!     │       ├── Create ResourceProcessor + register to PROCESSOR_REGISTRY
//!     │       └── Create FileSystemResourceController
//!     │
//!     ├── spawn::<Gateway, _>(GatewayHandler)
//!     │       └── ...
//!     │
//!     └── Run FileSystemWatcher (init phase + runtime phase)
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
use super::resource_controller::FileSystemResourceController;
use super::status::FileSystemStatusHandler;
use crate::core::controller::conf_mgr::conf_center::non_kube_common::{
    spawn_all_controllers, LocalControllerBackend,
};
use crate::core::controller::conf_mgr::conf_center::EndpointMode;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    ResourceProcessor, SecretRefManager,
};
use crate::core::controller::conf_mgr::sync_runtime::ShutdownSignal;
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::types::ResourceMeta;

/// Backend implementation that wires `FileSystemWatcher` and
/// `FileSystemResourceController` into the shared [`LocalControllerBackend`]
/// interface.
struct FileSystemControllerBackend {
    watcher: Arc<FileSystemWatcher>,
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
        K: ResourceMeta
            + Resource
            + Clone
            + Send
            + Sync
            + Debug
            + Serialize
            + DeserializeOwned
            + 'static,
    {
        let conf_dir = self.watcher.conf_dir().to_path_buf();
        let ctrl = FileSystemResourceController::new(kind, processor, conf_dir, event_rx)
            .with_shutdown(shutdown_signal);
        tokio::spawn(async move { ctrl.run().await })
    }
}

/// FileSystemController - Top-level controller for FileSystem mode
pub struct FileSystemController {
    conf_dir: PathBuf,
    endpoint_mode: EndpointMode,
}

impl FileSystemController {
    /// Create a new FileSystemController
    pub fn new(conf_dir: PathBuf, endpoint_mode: EndpointMode) -> Self {
        tracing::info!(
            component = "fs_controller",
            conf_dir = %conf_dir.display(),
            endpoint_mode = ?endpoint_mode,
            "Creating FileSystemController"
        );

        Self {
            conf_dir,
            endpoint_mode,
        }
    }

    /// Run the controller
    pub async fn run(&self, shutdown_signal: ShutdownSignal, runtime: Arc<ControllerRuntime>) -> Result<()> {
        tracing::info!(
            component = "fs_controller",
            conf_dir = %self.conf_dir.display(),
            "Starting FileSystemController"
        );

        // Cleanup orphan .status files at startup
        let status_handler = FileSystemStatusHandler::new(self.conf_dir.clone());
        match status_handler.cleanup_orphans() {
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
        let watcher = Arc::new(FileSystemWatcher::new(self.conf_dir.clone()));

        let backend = FileSystemControllerBackend {
            watcher: watcher.clone(),
        };

        // Spawn all resource controllers
        let handles = spawn_all_controllers(
            &backend,
            self.endpoint_mode,
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

// NOTE: The following items have been extracted to shared modules:
//   - `DEFAULT_CONTROLLER_NAME`  -> `common::DEFAULT_CONTROLLER_NAME`
//   - `phase1_kinds()`           -> `common::phase1_kinds`
//   - `spawn_all_controllers()`  -> `non_kube_common::spawn_all_controllers`
//   - `spawn()`                  -> `non_kube_common::spawn`
