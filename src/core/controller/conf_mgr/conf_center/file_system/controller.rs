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

use super::file_watcher::FileSystemWatcher;
use super::resource_controller::FileSystemResourceController;
use super::status::FileSystemStatusHandler;
use crate::core::controller::conf_mgr::conf_center::EndpointMode;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    ProcessorHandler, ResourceProcessor, SecretRefManager,
};
use crate::core::controller::conf_mgr::sync_runtime::ShutdownSignal;
use crate::core::controller::conf_mgr::PROCESSOR_REGISTRY;
use crate::types::prelude_resources::*;
use crate::types::ResourceMeta;
use anyhow::Result;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

// Import handlers
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    BackendTlsPolicyHandler, EdgionAcmeHandler, EdgionGatewayConfigHandler, EdgionPluginsHandler,
    EdgionStreamPluginsHandler, EdgionTlsHandler, EndpointSliceHandler, EndpointsHandler, GatewayClassHandler,
    GatewayHandler, GrpcRouteHandler, HttpRouteHandler, LinkSysHandler, PluginMetadataHandler, ReferenceGrantHandler,
    SecretHandler, ServiceHandler, TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
};

const DEFAULT_CONTROLLER_NAME: &str = "edgion.io/gateway-controller";

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
    pub async fn run(&self, shutdown_signal: ShutdownSignal) -> Result<()> {
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

        // Spawn all resource controllers
        let handles = self
            .spawn_all_controllers(&watcher, &secret_ref_manager, shutdown_signal.clone())
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

    /// Phase 1 foundation resource kinds for FileSystem mode.
    fn phase1_kinds(&self) -> Vec<&'static str> {
        let mut kinds = vec!["GatewayClass", "Gateway", "Secret", "ReferenceGrant", "Service"];
        if self.endpoint_mode.uses_endpoint() {
            kinds.push("Endpoints");
        }
        if self.endpoint_mode.uses_endpoint_slice() {
            kinds.push("EndpointSlice");
        }
        kinds
    }

    /// Spawn all resource controllers with phased initialization.
    ///
    /// Phase 1 (Foundation): GatewayClass, Gateway, Secret, ReferenceGrant,
    ///   Service, Endpoints/EndpointSlice
    /// Phase 2 (Dependent): Routes, EdgionTls, BackendTLSPolicy, Plugins, etc.
    ///
    /// Phase 2 waits for Phase 1 processors to complete init before spawning.
    async fn spawn_all_controllers(
        &self,
        watcher: &Arc<FileSystemWatcher>,
        secret_ref_manager: &Arc<SecretRefManager>,
        shutdown_signal: ShutdownSignal,
    ) -> Result<Vec<JoinHandle<Result<()>>>> {
        let mut handles = Vec::new();

        // ==================== Phase 1: Foundation Resources ====================
        tracing::info!(
            component = "fs_controller",
            "Phase 1: Spawning foundation resource controllers"
        );

        // Cluster-scoped foundation
        handles.push(
            spawn::<GatewayClass, _>(
                "GatewayClass",
                GatewayClassHandler::new(DEFAULT_CONTROLLER_NAME.to_string()),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // Gateway
        handles.push(
            spawn::<Gateway, _>(
                "Gateway",
                GatewayHandler::new(None, None),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // Secret
        handles.push(
            spawn::<Secret, _>(
                "Secret",
                SecretHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // ReferenceGrant
        handles.push(
            spawn::<ReferenceGrant, _>(
                "ReferenceGrant",
                ReferenceGrantHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // Service
        handles.push(
            spawn::<Service, _>(
                "Service",
                ServiceHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // Endpoint resources
        if self.endpoint_mode.uses_endpoint() {
            tracing::info!(
                component = "fs_controller",
                mode = ?self.endpoint_mode,
                "Registering Endpoints controller (Phase 1)"
            );
            handles.push(
                spawn::<Endpoints, _>(
                    "Endpoints",
                    EndpointsHandler::new(),
                    watcher,
                    secret_ref_manager,
                    shutdown_signal.clone(),
                )
                .await,
            );
        }

        if self.endpoint_mode.uses_endpoint_slice() {
            tracing::info!(
                component = "fs_controller",
                mode = ?self.endpoint_mode,
                "Registering EndpointSlice controller (Phase 1)"
            );
            handles.push(
                spawn::<EndpointSlice, _>(
                    "EndpointSlice",
                    EndpointSliceHandler::new(),
                    watcher,
                    secret_ref_manager,
                    shutdown_signal.clone(),
                )
                .await,
            );
        }

        let phase1_count = handles.len();
        tracing::info!(
            component = "fs_controller",
            count = phase1_count,
            "Phase 1 foundation controllers spawned, waiting for init completion"
        );

        // Wait for Phase 1 resources to complete their init phase
        const PHASE1_TIMEOUT: Duration = Duration::from_secs(15);
        let phase1_ready = PROCESSOR_REGISTRY
            .wait_kinds_ready(&self.phase1_kinds(), PHASE1_TIMEOUT)
            .await;

        if phase1_ready {
            tracing::info!(
                component = "fs_controller",
                "Phase 1 complete: all foundation resources ready, starting Phase 2"
            );
        } else {
            tracing::warn!(
                component = "fs_controller",
                "Phase 1 timeout: starting Phase 2 anyway (fallback to parallel init)"
            );
        }

        // ==================== Phase 2: Dependent Resources ====================
        tracing::info!(
            component = "fs_controller",
            "Phase 2: Spawning dependent resource controllers"
        );

        // Route resources
        handles.push(
            spawn::<HTTPRoute, _>(
                "HTTPRoute",
                HttpRouteHandler::new(DEFAULT_CONTROLLER_NAME.to_string()),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<GRPCRoute, _>(
                "GRPCRoute",
                GrpcRouteHandler::new(DEFAULT_CONTROLLER_NAME.to_string()),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<TCPRoute, _>(
                "TCPRoute",
                TcpRouteHandler::new(DEFAULT_CONTROLLER_NAME.to_string()),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<UDPRoute, _>(
                "UDPRoute",
                UdpRouteHandler::new(DEFAULT_CONTROLLER_NAME.to_string()),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<TLSRoute, _>(
                "TLSRoute",
                TlsRouteHandler::new(DEFAULT_CONTROLLER_NAME.to_string()),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // TLS related
        handles.push(
            spawn::<EdgionTls, _>(
                "EdgionTls",
                EdgionTlsHandler::new(DEFAULT_CONTROLLER_NAME.to_string()),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<BackendTLSPolicy, _>(
                "BackendTLSPolicy",
                BackendTlsPolicyHandler::new(DEFAULT_CONTROLLER_NAME.to_string()),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // Plugin resources
        handles.push(
            spawn::<EdgionPlugins, _>(
                "EdgionPlugins",
                EdgionPluginsHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<EdgionStreamPlugins, _>(
                "EdgionStreamPlugins",
                EdgionStreamPluginsHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<PluginMetaData, _>(
                "PluginMetaData",
                PluginMetadataHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<LinkSys, _>(
                "LinkSys",
                LinkSysHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // ACME
        handles.push(
            spawn::<EdgionAcme, _>(
                "EdgionAcme",
                EdgionAcmeHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // Cluster-scoped dependent
        handles.push(
            spawn::<EdgionGatewayConfig, _>(
                "EdgionGatewayConfig",
                EdgionGatewayConfigHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        tracing::info!(
            component = "fs_controller",
            phase1_count = phase1_count,
            phase2_count = handles.len() - phase1_count,
            total = handles.len(),
            "All resource controllers spawned (Phase 1 + Phase 2)"
        );

        Ok(handles)
    }
}

/// Spawn a FileSystemResourceController for a resource type
async fn spawn<K, H>(
    kind: &'static str,
    handler: H,
    watcher: &Arc<FileSystemWatcher>,
    secret_ref_manager: &Arc<SecretRefManager>,
    shutdown_signal: ShutdownSignal,
) -> JoinHandle<Result<()>>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    H: ProcessorHandler<K> + 'static,
{
    // 1. Create ResourceProcessor with capacity from config
    let capacity = crate::core::common::config::get_cache_capacity(kind);
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        capacity,
        Arc::new(handler),
        secret_ref_manager.clone(),
    ));

    // 2. Register to PROCESSOR_REGISTRY
    PROCESSOR_REGISTRY.register(processor.clone());

    // 3. Subscribe to watcher events for this kind
    let event_rx = watcher.subscribe(kind).await;

    // 4. Create and run FileSystemResourceController
    let conf_dir = watcher.conf_dir().to_path_buf();
    let ctrl = FileSystemResourceController::new(kind, processor, conf_dir, event_rx).with_shutdown(shutdown_signal);

    tokio::spawn(async move { ctrl.run().await })
}
