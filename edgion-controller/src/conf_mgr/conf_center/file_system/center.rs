//! FileSystemCenter - Unified configuration center for FileSystem mode
//!
//! Implements both CenterApi (CRUD) and CenterLifeCycle (lifecycle management),
//! automatically getting ConfCenter via blanket impl.
//!
//! Shared lifecycle logic is delegated to `CenterBase` (see `non_kube_common`).
//! The only FileSystem-specific work is creating `FileSystemController` per iteration.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

use super::config::FileSystemConfig;
use super::controller::FileSystemController;
use super::path_registry::PathRegistry;
use super::storage::FileSystemStorage;
use crate::conf_mgr::conf_center::non_kube_common::CenterBase;
use crate::conf_mgr::conf_center::traits::{
    impl_center_api_delegate, CenterApi, CenterLifeCycle, ConfWriterError, ListOptions, ListResult, ResourceKey,
};
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use crate::conf_mgr::ControllerRuntime;
use crate::conf_sync::conf_server::ConfigSyncServer;

/// FileSystemCenter - Configuration center for FileSystem mode
///
/// This struct implements both `CenterApi` and `CenterLifeCycle`,
/// automatically getting `ConfCenter` implementation via blanket impl.
///
/// # Single-node only (spec §11)
///
/// FileSystem mode is designed for local development and single-machine
/// testing. It has **no distributed coordination medium** — no lease, no lock,
/// no leader election — so running multiple Controller instances against the
/// same conf directory silently corrupts state (overlapping watchers, racing
/// `.status` writes, conflicting conf_sync snapshots). Deploy exactly one
/// instance. HA deployments must use the Kubernetes or Etcd backend, both of
/// which elect a single active leader.
pub struct FileSystemCenter {
    /// Configuration
    config: FileSystemConfig,
    /// Writer for CRUD operations (delegate)
    writer: FileSystemStorage,
    /// Shared path registry — single instance owned here; cloned into storage and controller
    registry: Arc<PathRegistry>,
    /// Shared lifecycle base (handles, runtime)
    base: CenterBase,
    /// Absolute conf directory, computed once so storage and the watcher share
    /// one identical scan/record base (design §0).
    conf_dir_abs: PathBuf,
}

impl FileSystemCenter {
    /// Create a new FileSystemCenter
    pub fn new(config: FileSystemConfig) -> Result<Self> {
        let registry = Arc::new(PathRegistry::new());
        // §0: absolutize once at the single source. Storage's scan base and the
        // watcher's recorded paths must be byte-identical or the list drop-loser
        // filter is a silent no-op under a relative conf_dir.
        let conf_dir_abs = super::storage::absolutize_path(config.conf_dir().clone());

        // Log the resolved absolute path so the startup log shows what storage
        // and the watcher actually use, not the operator's (possibly relative) input.
        tracing::info!(
            component = "file_system_center",
            mode = "file_system",
            conf_dir = %conf_dir_abs.display(),
            "Creating FileSystemCenter"
        );

        let writer = FileSystemStorage::new(conf_dir_abs.clone()).with_registry(registry.clone());

        Ok(Self {
            config,
            writer,
            registry,
            base: CenterBase::new(),
            conf_dir_abs,
        })
    }

    #[cfg(test)]
    pub(super) fn conf_dir_abs(&self) -> &std::path::Path {
        &self.conf_dir_abs
    }

    #[cfg(test)]
    pub(super) fn storage_root(&self) -> &std::path::Path {
        self.writer.root().as_path()
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
        // Spec §11: FileSystem mode has no distributed coordination medium, so
        // multi-instance deployments silently corrupt state. Say so at startup.
        tracing::warn!(
            component = "file_system_center",
            "FileSystem mode is single-node only: it has no lease/lock/leader election, and multiple \
             Controller instances sharing one conf directory silently corrupt state. Use the \
             Kubernetes or Etcd backend for HA."
        );

        // Inject conf_writer into runtime so handlers can auto-create resources (e.g. EdgionConfigData).
        let writer_arc: Arc<dyn CenterApi + Send + Sync> =
            Arc::new(FileSystemStorage::new(self.conf_dir_abs.clone()).with_registry(self.registry.clone()));
        self.base.runtime().set_conf_writer(writer_arc);

        let conf_dir = self.conf_dir_abs.clone();
        let registry = self.registry.clone();
        let poll_interval_override = self.config.poll_interval_ms.map(Duration::from_millis);

        self.base
            .run_lifecycle(
                shutdown_handle,
                "file_system_center",
                move |shutdown, runtime, error_tx| {
                    let ctrl = FileSystemController::new(conf_dir.clone(), registry.clone(), poll_interval_override);
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

    fn conf_dir_registry(&self) -> Option<Arc<super::path_registry::PathRegistry>> {
        Some(self.registry.clone())
    }
}

#[cfg(test)]
mod center_path_tests {
    use super::*;

    #[test]
    fn new_absolutizes_conf_dir_and_shares_base_with_storage() {
        let center = FileSystemCenter::new(FileSystemConfig::new(std::path::PathBuf::from("relative/conf"))).unwrap();
        assert!(
            center.conf_dir_abs().is_absolute(),
            "conf_dir handed to the watcher must be absolute"
        );
        assert_eq!(
            center.storage_root(),
            center.conf_dir_abs(),
            "storage scan base must equal the watcher base (design §0)"
        );
    }
}
