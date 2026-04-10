//! FileSystemCenter - Unified configuration center for FileSystem mode
//!
//! Implements both CenterApi (CRUD) and CenterLifeCycle (lifecycle management),
//! automatically getting ConfCenter via blanket impl.
//!
//! Shared lifecycle logic is delegated to `CenterBase` (see `non_kube_common`).
//! The only FileSystem-specific work is creating `FileSystemController` per iteration.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::config::FileSystemConfig;
use super::controller::FileSystemController;
use super::storage::FileSystemStorage;
use crate::core::controller::conf_mgr::conf_center::non_kube_common::{resolve_endpoint_mode, CenterBase};
use crate::core::controller::conf_mgr::conf_center::traits::{
    impl_center_api_delegate, CenterApi, CenterLifeCycle, ConfWriterError, ListOptions, ListResult,
};
use crate::core::controller::conf_mgr::sync_runtime::ShutdownHandle;
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::core::controller::conf_sync::conf_server::ConfigSyncServer;

/// FileSystemCenter - Configuration center for FileSystem mode
///
/// This struct implements both `CenterApi` and `CenterLifeCycle`,
/// automatically getting `ConfCenter` implementation via blanket impl.
pub struct FileSystemCenter {
    /// Configuration
    config: FileSystemConfig,
    /// Writer for CRUD operations (delegate)
    writer: FileSystemStorage,
    /// Shared lifecycle base (handles, runtime)
    base: CenterBase,
}

impl FileSystemCenter {
    /// Create a new FileSystemCenter
    pub fn new(config: FileSystemConfig) -> Result<Self> {
        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            conf_dir = %config.conf_dir().display(),
            "Creating FileSystemCenter"
        );

        let writer = FileSystemStorage::new(config.conf_dir());

        Ok(Self {
            config,
            writer,
            base: CenterBase::new(),
        })
    }

    /// Get the configuration
    pub fn config(&self) -> &FileSystemConfig {
        &self.config
    }
}

// CenterApi implementation - delegates to `writer` via macro
impl_center_api_delegate!(FileSystemCenter, writer);

// ============================================================================
// CenterLifeCycle implementation - FileSystem lifecycle logic
// ============================================================================

#[async_trait]
impl CenterLifeCycle for FileSystemCenter {
    /// FileSystem mode lifecycle with external shutdown handle
    ///
    /// Uses the new FileSystemController for unified init + runtime flow:
    /// 1. Resolve endpoint mode
    /// 2. Run FileSystemController (registers to PROCESSOR_REGISTRY, runs init + runtime)
    /// 3. Wait for PROCESSOR_REGISTRY to be ready
    /// 4. Create ConfigSyncServer and register WatchObjs
    /// 5. Set config_sync_server = Some (services become available)
    /// 6. Wait for shutdown signal or reload request
    ///
    /// Supports reload: when reload is requested, the controller is restarted
    /// and a new ConfigSyncServer is created (with new server_id).
    async fn start(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            "Starting FileSystem lifecycle"
        );

        let endpoint_mode = resolve_endpoint_mode(self.config.endpoint_mode());
        let conf_dir = self.config.conf_dir().clone();

        self.base
            .run_lifecycle(
                shutdown_handle,
                endpoint_mode,
                "file_system_center",
                |shutdown, runtime, error_tx| {
                    let ctrl = FileSystemController::new(conf_dir.clone(), endpoint_mode);
                    tokio::spawn(async move {
                        if let Err(e) = ctrl.run(shutdown, runtime).await {
                            tracing::error!(
                                component = "file_system_center",
                                error = %e,
                                "FileSystemController error"
                            );
                            let _ = error_tx.send(e.to_string());
                        }
                    })
                },
            )
            .await
    }

    /// Check if the system is ready
    fn is_ready(&self) -> bool {
        self.base.is_ready()
    }

    /// Get the ConfigSyncServer (may be None if not ready)
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        self.base.config_sync_server()
    }

    /// Check if running in Kubernetes mode
    fn is_k8s_mode(&self) -> bool {
        false
    }

    /// Request a reload (re-initialize all processors and stores)
    fn request_reload(&self) -> Result<(), String> {
        self.base.request_reload()
    }

    fn runtime(&self) -> Arc<ControllerRuntime> {
        self.base.runtime()
    }
}

// FileSystemCenter automatically implements ConfCenter via blanket impl
// because it implements both CenterApi and CenterLifeCycle
