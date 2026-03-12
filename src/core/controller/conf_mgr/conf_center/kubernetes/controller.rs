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
//! │           └── ...  ...                                               │
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
use futures::TryStreamExt;
use k8s_openapi::api::core::v1::{Endpoints, Namespace, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::watcher;
use kube::runtime::WatchStreamExt;
use kube::{Client, Resource, ResourceExt};
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
use crate::core::controller::conf_mgr::conf_center::{EndpointMode, MetadataFilterConfig};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    ProcessorHandler, ResourceProcessor, SecretRefManager,
};
use crate::core::controller::conf_mgr::PROCESSOR_REGISTRY;
use crate::types::prelude_resources::*;
use crate::types::ResourceMeta;

// Import handlers from conf_mgr
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    BackendTlsPolicyHandler, EdgionAcmeHandler, EdgionGatewayConfigHandler, EdgionPluginsHandler,
    EdgionStreamPluginsHandler, EdgionTlsHandler, EndpointSliceHandler, EndpointsHandler, GatewayClassHandler,
    GatewayHandler, GrpcRouteHandler, HttpRouteHandler, LinkSysHandler, PluginMetadataHandler, ReferenceGrantHandler,
    SecretHandler, ServiceHandler, TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
};

/// Context for spawn functions
struct SpawnContext {
    watcher_config: watcher::Config,
    shutdown: ShutdownSignal,
    relink_tx: RelinkSignalSender,
    secret_ref_manager: Arc<SecretRefManager>,
    metadata_filter: Option<MetadataFilterConfig>,
    leader_handle: Option<super::leader_election::LeaderHandle>,
}

/// Spawn a namespaced ResourceController with the given handler
fn spawn<K, H>(
    controller: &KubernetesController,
    kind: &'static str,
    handler: H,
    ctx: &SpawnContext,
) -> JoinHandle<Result<()>>
where
    K: ResourceMeta
        + Resource<Scope = kube::core::NamespaceResourceScope>
        + Clone
        + Send
        + Sync
        + Debug
        + Serialize
        + DeserializeOwned
        + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    H: ProcessorHandler<K> + 'static,
{
    // 1. Create ResourceProcessor with capacity from config
    let capacity = crate::core::common::config::get_cache_capacity(kind);
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        capacity,
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
    let mut rc = ResourceController::new(
        kind,
        controller.client.clone(),
        processor,
        ApiScope::Namespaced(controller.watch_mode.clone()),
        ctx.watcher_config.clone(),
    )
    .with_shutdown(ctx.shutdown.clone())
    .with_relink_signal(ctx.relink_tx.clone());

    if let Some(ref lh) = ctx.leader_handle {
        rc = rc.with_leader_handle(lh.clone());
    }

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
    K: ResourceMeta
        + Resource<Scope = kube::core::ClusterResourceScope>
        + Clone
        + Send
        + Sync
        + Debug
        + Serialize
        + DeserializeOwned
        + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    H: ProcessorHandler<K> + 'static,
{
    // 1. Create ResourceProcessor with capacity from config
    let capacity = crate::core::common::config::get_cache_capacity(kind);
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        capacity,
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
    let mut rc = ResourceController::new(
        kind,
        controller.client.clone(),
        processor,
        ApiScope::ClusterScoped,
        ctx.watcher_config.clone(),
    )
    .with_shutdown(ctx.shutdown.clone())
    .with_relink_signal(ctx.relink_tx.clone());

    if let Some(ref lh) = ctx.leader_handle {
        rc = rc.with_leader_handle(lh.clone());
    }

    tokio::spawn(async move { rc.run_cluster_scoped().await })
}

/// Spawn a lightweight Namespace label watcher.
///
/// Unlike other resources, Namespace does not go through the ResourceProcessor
/// pipeline. It only populates the global NamespaceStore for Selector
/// namespace policy evaluation.
fn spawn_namespace_watcher(
    client: Client,
    watcher_config: watcher::Config,
    shutdown: ShutdownSignal,
) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        let api: kube::Api<Namespace> = kube::Api::all(client);
        let store =
            crate::core::controller::conf_mgr::sync_runtime::resource_processor::namespace_store::get_namespace_store();

        tracing::info!(component = "namespace_watcher", "Starting Namespace label watcher");

        let stream = watcher::watcher(api, watcher_config)
            .default_backoff()
            .applied_objects();

        futures::pin_mut!(stream);

        let mut shutdown = shutdown;
        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    tracing::info!(component = "namespace_watcher", "Shutdown signal received");
                    break;
                }
                event = stream.try_next() => {
                    match event {
                        Ok(Some(ns)) => {
                            let name = ns.name_any();
                            let changed = if ns.metadata.deletion_timestamp.is_some() {
                                store.remove(&name)
                            } else {
                                store.upsert(ns)
                            };
                            if changed {
                                tracing::debug!(
                                    component = "namespace_watcher",
                                    namespace = %name,
                                    "Namespace labels changed, requeuing Selector Gateways"
                                );
                                requeue_selector_gateways();
                            }
                        }
                        Ok(None) => {
                            tracing::warn!(component = "namespace_watcher", "Watch stream ended");
                            break;
                        }
                        Err(e) => {
                            tracing::error!(component = "namespace_watcher", error = %e, "Watch error");
                        }
                    }
                }
            }
        }
        Ok(())
    })
}

/// Requeue all Gateways that use Selector namespace policy so they
/// re-evaluate listener namespace constraints.
fn requeue_selector_gateways() {
    let Some(processor) = PROCESSOR_REGISTRY.get("Gateway") else {
        return;
    };
    let Ok((json, _)) = processor.as_watch_obj().list_json() else {
        return;
    };
    let gateways: Vec<crate::types::prelude_resources::Gateway> = match serde_json::from_str(&json) {
        Ok(g) => g,
        Err(_) => return,
    };
    for gw in &gateways {
        let uses_selector = gw.spec.listeners.as_deref().unwrap_or_default().iter().any(|l| {
            l.allowed_routes
                .as_ref()
                .and_then(|ar| ar.namespaces.as_ref())
                .and_then(|ns| ns.from.as_deref())
                == Some("Selector")
        });
        if uses_selector {
            let key = format!(
                "{}/{}",
                gw.metadata.namespace.as_deref().unwrap_or("default"),
                gw.metadata.name.as_deref().unwrap_or("")
            );
            PROCESSOR_REGISTRY.requeue("Gateway", key);
        }
    }
}

/// Kubernetes Controller that spawns independent ResourceControllers for each resource type
///
/// Note: Leader election is handled externally by lifecycle_kubernetes.rs.
/// This controller focuses solely on resource watching and synchronization.
pub struct KubernetesController {
    client: Client,
    gateway_class_name: String,
    controller_name: String,
    gateway_address: Option<String>,
    watch_mode: NamespaceWatchMode,
    label_selector: Option<String>,
    /// Optional metadata filter configuration for reducing resource memory usage
    metadata_filter: Option<MetadataFilterConfig>,
    /// Resolved endpoint mode (Auto should be resolved before controller creation)
    endpoint_mode: EndpointMode,
    /// Leader handle for gating status writes in all-serve HA mode
    leader_handle: Option<super::leader_election::LeaderHandle>,
}

impl KubernetesController {
    /// Create a new KubernetesController
    pub async fn new(
        gateway_class_name: String,
        controller_name: String,
        gateway_address: Option<String>,
        watch_namespaces: Vec<String>,
        label_selector: Option<String>,
        endpoint_mode: EndpointMode,
    ) -> Result<Self> {
        let client = Client::try_default().await?;
        Self::with_metadata_filter(
            client,
            gateway_class_name,
            controller_name,
            gateway_address,
            watch_namespaces,
            label_selector,
            MetadataFilterConfig::default(),
            endpoint_mode,
        )
    }

    /// Create a new KubernetesController with metadata filter
    ///
    /// Accepts an external Client to enable Client reuse across components.
    #[allow(clippy::too_many_arguments)]
    pub fn with_metadata_filter(
        client: Client,
        gateway_class_name: String,
        controller_name: String,
        gateway_address: Option<String>,
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
            controller_name = %controller_name,
            gateway_address = ?gateway_address,
            metadata_filter_enabled = true,
            endpoint_mode = ?endpoint_mode,
            "Creating Kubernetes controller"
        );

        Ok(Self {
            client,
            gateway_class_name,
            controller_name,
            gateway_address,
            watch_mode,
            label_selector,
            metadata_filter: Some(metadata_filter),
            endpoint_mode,
            leader_handle: None,
        })
    }

    /// Set leader handle for gating status writes in all-serve HA mode
    pub fn with_leader_handle(mut self, handle: super::leader_election::LeaderHandle) -> Self {
        self.leader_handle = Some(handle);
        self
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
            leader_handle: self.leader_handle.clone(),
        };

        let mut h = Vec::new();

        // ==================== Namespaced Resources ====================
        // Route resources
        h.push(spawn::<HTTPRoute, _>(
            self,
            "HTTPRoute",
            HttpRouteHandler::new(self.controller_name.clone()),
            &ctx,
        ));
        h.push(spawn::<GRPCRoute, _>(
            self,
            "GRPCRoute",
            GrpcRouteHandler::new(self.controller_name.clone()),
            &ctx,
        ));
        h.push(spawn::<TCPRoute, _>(
            self,
            "TCPRoute",
            TcpRouteHandler::new(self.controller_name.clone()),
            &ctx,
        ));
        h.push(spawn::<UDPRoute, _>(
            self,
            "UDPRoute",
            UdpRouteHandler::new(self.controller_name.clone()),
            &ctx,
        ));
        h.push(spawn::<TLSRoute, _>(
            self,
            "TLSRoute",
            TlsRouteHandler::new(self.controller_name.clone()),
            &ctx,
        ));

        // Backend resources
        h.push(spawn::<Service, _>(self, "Service", ServiceHandler::new(), &ctx));

        // Register endpoint handlers based on endpoint mode
        if self.endpoint_mode.uses_endpoint() {
            tracing::info!(
                component = "k8s_controller",
                mode = ?self.endpoint_mode,
                "Registering Endpoints controller"
            );
            h.push(spawn::<Endpoints, _>(self, "Endpoints", EndpointsHandler::new(), &ctx));
        }

        if self.endpoint_mode.uses_endpoint_slice() {
            tracing::info!(
                component = "k8s_controller",
                mode = ?self.endpoint_mode,
                "Registering EndpointSlice controller"
            );
            h.push(spawn::<EndpointSlice, _>(
                self,
                "EndpointSlice",
                EndpointSliceHandler::new(),
                &ctx,
            ));
        }

        // Safety check: Auto mode should have been resolved
        if self.endpoint_mode.is_auto() {
            unreachable!("EndpointMode::Auto should be resolved before run_controllers");
        }

        // TLS related (special processing)
        h.push(spawn::<Secret, _>(self, "Secret", SecretHandler::new(), &ctx));
        h.push(spawn::<EdgionTls, _>(
            self,
            "EdgionTls",
            EdgionTlsHandler::new(self.controller_name.clone()),
            &ctx,
        ));
        h.push(spawn::<BackendTLSPolicy, _>(
            self,
            "BackendTLSPolicy",
            BackendTlsPolicyHandler::new(self.controller_name.clone()),
            &ctx,
        ));

        // Gateway (special processing: filter by gateway_class)
        h.push(spawn::<Gateway, _>(
            self,
            "Gateway",
            GatewayHandler::new(Some(self.gateway_class_name.clone()), self.gateway_address.clone()),
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

        // ==================== ACME Resources ====================
        h.push(spawn::<EdgionAcme, _>(
            self,
            "EdgionAcme",
            EdgionAcmeHandler::new(),
            &ctx,
        ));

        // ==================== Cluster-Scoped Resources ====================
        h.push(spawn_cluster::<GatewayClass, _>(
            self,
            "GatewayClass",
            GatewayClassHandler::new(self.controller_name.clone()),
            &ctx,
        ));
        h.push(spawn_cluster::<EdgionGatewayConfig, _>(
            self,
            "EdgionGatewayConfig",
            EdgionGatewayConfigHandler::new(),
            &ctx,
        ));

        // ==================== Auxiliary Watchers ====================
        h.push(spawn_namespace_watcher(
            self.client.clone(),
            watcher::Config::default(),
            ctx.shutdown.clone(),
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
