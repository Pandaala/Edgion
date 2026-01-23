//! Kubernetes Controller using Go operator-style Workqueue
//!
//! This module implements a Kubernetes controller where each resource type runs
//! as a **completely independent** ResourceController with its own lifecycle.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                    KubernetesController.run()                                │
//! │                    (Spawns independent ResourceControllers)                  │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │                                                                              │
//! │   spawn ──┬── HTTPRoute ─────────────────────────────────────────────────►  │
//! │           │     1. Create ResourceProcessor<HTTPRoute>                       │
//! │           │     2. Register to PROCESSOR_REGISTRY                            │
//! │           │     3. Create ResourceController                                 │
//! │           │     4. Run (Init → Runtime)                                      │
//! │           │                                                                  │
//! │           ├── Gateway ───────────────────────────────────────────────────►  │
//! │           │     [Same flow + gateway_class filter]                          │
//! │           │                                                                  │
//! │           └── ... 其他资源 ...                                               │
//! │                                                                              │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Decisions
//!
//! 1. **Complete Independence**: Each resource type runs its own flow independently.
//! 2. **Processor Registration**: Each processor is registered to PROCESSOR_REGISTRY on spawn.
//! 3. **Parallel Initialization**: All resource types initialize in parallel.
//! 4. **No ConfigServer**: Processor manages its own ServerCache internally.
//! 5. **Graceful Shutdown**: Handles SIGTERM/SIGINT for clean shutdown.

use anyhow::Result;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::watcher;
use kube::{Client, Resource};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::namespace::NamespaceWatchMode;
use super::resource_controller::{ApiScope, RelinkReason, RelinkSignalSender, ResourceController};
use super::ShutdownSignal;
use crate::core::conf_mgr::conf_center::EndpointMode;
use crate::core::conf_mgr::MetadataFilterConfig;
use crate::core::conf_mgr_new::sync_runtime::resource_processor::{
    ProcessorHandler, ResourceProcessor, SecretRefManager,
};
use crate::core::conf_mgr_new::PROCESSOR_REGISTRY;
use crate::types::prelude_resources::*;
use crate::types::ResourceMeta;

// Import handlers from conf_mgr_new
use crate::core::conf_mgr_new::sync_runtime::resource_processor::{
    BackendTlsPolicyHandler, EdgionGatewayConfigHandler, EdgionPluginsHandler, EdgionStreamPluginsHandler,
    EdgionTlsHandler, EndpointSliceHandler, EndpointsHandler, GatewayClassHandler, GatewayHandler, GrpcRouteHandler,
    HttpRouteHandler, LinkSysHandler, PluginMetadataHandler, ReferenceGrantHandler, SecretHandler, ServiceHandler,
    TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
};

/// Default cache capacity for each resource type
const DEFAULT_CACHE_CAPACITY: usize = 1000;

/// Context for spawn functions
struct SpawnContext {
    watcher_config: watcher::Config,
    shutdown: ShutdownSignal,
    relink_tx: RelinkSignalSender,
    secret_ref_manager: Arc<SecretRefManager>,
    metadata_filter: Option<MetadataFilterConfig>,
}

/// Spawn a namespaced ResourceController with the given handler
fn spawn<K, H>(
    controller: &KubernetesController,
    kind: &'static str,
    handler: H,
    ctx: &SpawnContext,
) -> JoinHandle<Result<()>>
where
    K: ResourceMeta + Resource<Scope = kube::core::NamespaceResourceScope> + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    H: ProcessorHandler<K> + 'static,
{
    // 1. Create ResourceProcessor
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        DEFAULT_CACHE_CAPACITY,
        Arc::new(handler),
        ctx.secret_ref_manager.clone(),
    ));

    // 2. Set metadata filter if configured
    if let Some(ref filter) = ctx.metadata_filter {
        processor.set_metadata_filter(filter.clone());
    }

    // 3. Register to PROCESSOR_REGISTRY
    PROCESSOR_REGISTRY.register(processor.clone());

    // 4. Create and run ResourceController
    let rc = ResourceController::new(
        kind,
        controller.client.clone(),
        processor,
        ApiScope::Namespaced(controller.watch_mode.clone()),
        ctx.watcher_config.clone(),
    )
    .with_shutdown(ctx.shutdown.clone())
    .with_relink_signal(ctx.relink_tx.clone());

    tokio::spawn(async move { rc.run_namespaced().await })
}

/// Spawn a cluster-scoped ResourceController with the given handler
fn spawn_cluster<K, H>(
    controller: &KubernetesController,
    kind: &'static str,
    handler: H,
    ctx: &SpawnContext,
) -> JoinHandle<Result<()>>
where
    K: ResourceMeta + Resource<Scope = kube::core::ClusterResourceScope> + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    H: ProcessorHandler<K> + 'static,
{
    // 1. Create ResourceProcessor
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        DEFAULT_CACHE_CAPACITY,
        Arc::new(handler),
        ctx.secret_ref_manager.clone(),
    ));

    // 2. Set metadata filter if configured
    if let Some(ref filter) = ctx.metadata_filter {
        processor.set_metadata_filter(filter.clone());
    }

    // 3. Register to PROCESSOR_REGISTRY
    PROCESSOR_REGISTRY.register(processor.clone());

    // 4. Create and run ResourceController
    let rc = ResourceController::new(
        kind,
        controller.client.clone(),
        processor,
        ApiScope::ClusterScoped,
        ctx.watcher_config.clone(),
    )
    .with_shutdown(ctx.shutdown.clone())
    .with_relink_signal(ctx.relink_tx.clone());

    tokio::spawn(async move { rc.run_cluster_scoped().await })
}

/// Kubernetes Controller that spawns independent ResourceControllers for each resource type
///
/// Note: Leader election is handled externally by lifecycle_kubernetes.rs.
/// This controller focuses solely on resource watching and synchronization.
pub struct KubernetesController {
    client: Client,
    gateway_class_name: String,
    watch_mode: NamespaceWatchMode,
    label_selector: Option<String>,
    /// Optional metadata filter configuration for reducing resource memory usage
    metadata_filter: Option<MetadataFilterConfig>,
    /// Resolved endpoint mode (Auto should be resolved before controller creation)
    endpoint_mode: EndpointMode,
}

impl KubernetesController {
    /// Create a new KubernetesController
    pub async fn new(
        gateway_class_name: String,
        watch_namespaces: Vec<String>,
        label_selector: Option<String>,
        endpoint_mode: EndpointMode,
    ) -> Result<Self> {
        let client = Client::try_default().await?;
        Self::with_metadata_filter(
            client,
            gateway_class_name,
            watch_namespaces,
            label_selector,
            MetadataFilterConfig::default(),
            endpoint_mode,
        )
    }

    /// Create a new KubernetesController with metadata filter
    ///
    /// Accepts an external Client to enable Client reuse across components.
    pub fn with_metadata_filter(
        client: Client,
        gateway_class_name: String,
        watch_namespaces: Vec<String>,
        label_selector: Option<String>,
        metadata_filter: MetadataFilterConfig,
        endpoint_mode: EndpointMode,
    ) -> Result<Self> {
        let watch_mode = NamespaceWatchMode::from_namespaces(watch_namespaces);

        tracing::info!(
            component = "k8s_controller",
            watch_mode = ?watch_mode,
            label_selector = ?label_selector,
            gateway_class_name = %gateway_class_name,
            metadata_filter_enabled = true,
            endpoint_mode = ?endpoint_mode,
            "Creating Kubernetes controller"
        );

        Ok(Self {
            client,
            gateway_class_name,
            watch_mode,
            label_selector,
            metadata_filter: Some(metadata_filter),
            endpoint_mode,
        })
    }

    /// Create watcher configuration with optional label selector
    fn watcher_config(&self) -> watcher::Config {
        let mut config = watcher::Config::default();
        if let Some(ref selector) = self.label_selector {
            config = config.labels(selector);
        }
        config
    }

    /// Run the controller - spawns independent ResourceControllers for all resource types
    ///
    /// Each ResourceController runs completely independently:
    /// - Creates its own ResourceProcessor (with ServerCache)
    /// - Registers processor to PROCESSOR_REGISTRY
    /// - Applies InitAdd for its resources
    /// - Marks its cache ready
    /// - Immediately starts workqueue reconcile loop
    ///
    /// Also handles:
    /// - Graceful shutdown via provided ShutdownSignal
    /// - 410 Gone detection and relink signaling
    pub async fn run(&self, shutdown_signal: ShutdownSignal) -> Result<ControllerExitReason> {
        self.run_controllers(shutdown_signal).await
    }

    /// Internal method to run all controllers
    /// Returns when shutdown is triggered or a relink signal is received
    #[allow(clippy::vec_init_then_push)]
    async fn run_controllers(&self, mut shutdown_signal: ShutdownSignal) -> Result<ControllerExitReason> {
        tracing::info!(
            component = "k8s_controller",
            "Starting Kubernetes controller - spawning independent ResourceControllers"
        );

        // Create relink signal channel
        let (relink_tx, mut relink_rx) = mpsc::channel::<RelinkReason>(10);

        // Create global SecretRefManager (shared by all ResourceControllers)
        let secret_ref_manager = Arc::new(SecretRefManager::new());

        // Create SpawnContext
        let ctx = SpawnContext {
            watcher_config: self.watcher_config(),
            shutdown: shutdown_signal.clone(),
            relink_tx: relink_tx.clone(),
            secret_ref_manager: secret_ref_manager.clone(),
            metadata_filter: self.metadata_filter.clone(),
        };

        let mut h = Vec::new();

        // ==================== Namespaced Resources ====================
        // Route resources
        h.push(spawn::<HTTPRoute, _>(self, "HTTPRoute", HttpRouteHandler::new(), &ctx));
        h.push(spawn::<GRPCRoute, _>(self, "GRPCRoute", GrpcRouteHandler::new(), &ctx));
        h.push(spawn::<TCPRoute, _>(self, "TCPRoute", TcpRouteHandler::new(), &ctx));
        h.push(spawn::<UDPRoute, _>(self, "UDPRoute", UdpRouteHandler::new(), &ctx));
        h.push(spawn::<TLSRoute, _>(self, "TLSRoute", TlsRouteHandler::new(), &ctx));

        // Backend resources
        h.push(spawn::<Service, _>(self, "Service", ServiceHandler::new(), &ctx));
        match self.endpoint_mode {
            EndpointMode::Endpoint => {
                tracing::info!(
                    component = "k8s_controller",
                    "Registering Endpoints controller (legacy mode)"
                );
                h.push(spawn::<Endpoints, _>(self, "Endpoints", EndpointsHandler::new(), &ctx));
            }
            EndpointMode::EndpointSlice => {
                tracing::info!(
                    component = "k8s_controller",
                    "Registering EndpointSlice controller (modern mode)"
                );
                h.push(spawn::<EndpointSlice, _>(
                    self,
                    "EndpointSlice",
                    EndpointSliceHandler::new(),
                    &ctx,
                ));
            }
            EndpointMode::Auto => {
                unreachable!("EndpointMode::Auto should be resolved before run_controllers");
            }
        }

        // TLS related (special processing)
        h.push(spawn::<Secret, _>(self, "Secret", SecretHandler::new(), &ctx));
        h.push(spawn::<EdgionTls, _>(self, "EdgionTls", EdgionTlsHandler::new(), &ctx));
        h.push(spawn::<BackendTLSPolicy, _>(
            self,
            "BackendTLSPolicy",
            BackendTlsPolicyHandler::new(),
            &ctx,
        ));

        // Gateway (special processing: filter by gateway_class)
        h.push(spawn::<Gateway, _>(
            self,
            "Gateway",
            GatewayHandler::new(Some(self.gateway_class_name.clone())),
            &ctx,
        ));

        // Other namespaced resources
        h.push(spawn::<ReferenceGrant, _>(
            self,
            "ReferenceGrant",
            ReferenceGrantHandler::new(),
            &ctx,
        ));
        h.push(spawn::<EdgionPlugins, _>(
            self,
            "EdgionPlugins",
            EdgionPluginsHandler::new(),
            &ctx,
        ));
        h.push(spawn::<EdgionStreamPlugins, _>(
            self,
            "EdgionStreamPlugins",
            EdgionStreamPluginsHandler::new(),
            &ctx,
        ));
        h.push(spawn::<PluginMetaData, _>(
            self,
            "PluginMetaData",
            PluginMetadataHandler::new(),
            &ctx,
        ));
        h.push(spawn::<LinkSys, _>(self, "LinkSys", LinkSysHandler::new(), &ctx));

        // ==================== Cluster-Scoped Resources ====================
        h.push(spawn_cluster::<GatewayClass, _>(
            self,
            "GatewayClass",
            GatewayClassHandler::new(),
            &ctx,
        ));
        h.push(spawn_cluster::<EdgionGatewayConfig, _>(
            self,
            "EdgionGatewayConfig",
            EdgionGatewayConfigHandler::new(),
            &ctx,
        ));

        // Drop our copy of the sender so we can detect when all controllers stop
        drop(relink_tx);

        tracing::info!(
            component = "k8s_controller",
            count = h.len(),
            "All ResourceControllers spawned - each running independently"
        );

        // Wait for either:
        // 1. Shutdown signal (Ctrl+C / SIGTERM)
        // 2. A relink signal (410 Gone detected)
        // 3. All controllers to stop
        let exit_reason = tokio::select! {
            _ = shutdown_signal.wait() => {
                tracing::info!(
                    component = "k8s_controller",
                    "Shutdown signal received in run_controllers"
                );
                ControllerExitReason::Shutdown
            }
            reason = relink_rx.recv() => {
                match reason {
                    Some(r) => {
                        tracing::warn!(
                            component = "k8s_controller",
                            reason = ?r,
                            "Received relink signal from ResourceController"
                        );
                        ControllerExitReason::RelinkRequested(r)
                    }
                    None => {
                        tracing::warn!(
                            component = "k8s_controller",
                            "Relink channel closed (all controllers stopped)"
                        );
                        ControllerExitReason::AllControllersStopped
                    }
                }
            }
            _ = futures::future::join_all(&mut h) => {
                tracing::warn!(
                    component = "k8s_controller",
                    "All controllers have stopped"
                );
                ControllerExitReason::AllControllersStopped
            }
        };

        Ok(exit_reason)
    }
}

/// Reason for controller exit
#[derive(Debug)]
pub enum ControllerExitReason {
    /// Shutdown was requested
    Shutdown,
    /// A relink was requested (e.g., 410 Gone detected)
    RelinkRequested(RelinkReason),
    /// All controllers stopped (unexpected)
    AllControllersStopped,
    /// Lost leadership
    LostLeadership,
}
