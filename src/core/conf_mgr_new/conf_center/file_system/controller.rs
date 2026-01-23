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
use crate::core::conf_mgr::conf_center::EndpointMode;
use crate::core::conf_mgr_new::sync_runtime::resource_processor::{
    ProcessorHandler, ResourceProcessor, SecretRefManager,
};
use crate::core::conf_mgr_new::sync_runtime::ShutdownSignal;
use crate::core::conf_mgr_new::PROCESSOR_REGISTRY;
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
use tokio::task::JoinHandle;

// Import handlers
use crate::core::conf_mgr_new::sync_runtime::resource_processor::{
    BackendTlsPolicyHandler, EdgionGatewayConfigHandler, EdgionPluginsHandler, EdgionStreamPluginsHandler,
    EdgionTlsHandler, EndpointSliceHandler, EndpointsHandler, GatewayClassHandler, GatewayHandler, GrpcRouteHandler,
    HttpRouteHandler, LinkSysHandler, PluginMetadataHandler, ReferenceGrantHandler, SecretHandler, ServiceHandler,
    TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
};

/// Default cache capacity for each resource type
const DEFAULT_CACHE_CAPACITY: usize = 1000;

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

        tracing::info!(
            component = "fs_controller",
            "FileSystemController stopped"
        );

        Ok(())
    }

    /// Spawn all resource controllers
    async fn spawn_all_controllers(
        &self,
        watcher: &Arc<FileSystemWatcher>,
        secret_ref_manager: &Arc<SecretRefManager>,
        shutdown_signal: ShutdownSignal,
    ) -> Result<Vec<JoinHandle<Result<()>>>> {
        let mut handles = Vec::new();

        // Route resources
        handles.push(
            spawn::<HTTPRoute, _>(
                "HTTPRoute",
                HttpRouteHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<GRPCRoute, _>(
                "GRPCRoute",
                GrpcRouteHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<TCPRoute, _>(
                "TCPRoute",
                TcpRouteHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<UDPRoute, _>(
                "UDPRoute",
                UdpRouteHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<TLSRoute, _>(
                "TLSRoute",
                TlsRouteHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // Backend resources
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

        match self.endpoint_mode {
            EndpointMode::Endpoint => {
                tracing::info!(
                    component = "fs_controller",
                    "Registering Endpoints controller (legacy mode)"
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
            EndpointMode::EndpointSlice | EndpointMode::Auto => {
                tracing::info!(
                    component = "fs_controller",
                    "Registering EndpointSlice controller"
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
        }

        // TLS related
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
        handles.push(
            spawn::<EdgionTls, _>(
                "EdgionTls",
                EdgionTlsHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
        handles.push(
            spawn::<BackendTLSPolicy, _>(
                "BackendTLSPolicy",
                BackendTlsPolicyHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // Gateway (no gateway_class filter in FileSystem mode)
        handles.push(
            spawn::<Gateway, _>(
                "Gateway",
                GatewayHandler::new(None),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );

        // Other resources
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

        // Cluster-scoped resources
        handles.push(
            spawn::<GatewayClass, _>(
                "GatewayClass",
                GatewayClassHandler::new(),
                watcher,
                secret_ref_manager,
                shutdown_signal.clone(),
            )
            .await,
        );
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
    // 1. Create ResourceProcessor
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        DEFAULT_CACHE_CAPACITY,
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
