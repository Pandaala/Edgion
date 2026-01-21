//! Kubernetes Controller using Go operator-style Workqueue
//!
//! This module implements a Kubernetes controller where each resource type runs
//! as a **completely independent** ResourceController with its own 1-8 lifecycle.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                    KubernetesController.run()                                │
//! │                    (Only spawns independent ResourceControllers)             │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │                                                                              │
//! │   tokio::spawn ──┬── HTTPRoute ResourceController ───────────────────────►  │
//! │                  │       [1-8 独立流程: create→reflector→wait→init→workqueue]│
//! │                  │                                                           │
//! │                  ├── GRPCRoute ResourceController ───────────────────────►  │
//! │                  │       [1-8 独立流程]                                       │
//! │                  │                                                           │
//! │                  ├── Gateway ResourceController ─────────────────────────►  │
//! │                  │       [1-8 独立流程 + gateway_class 过滤]                  │
//! │                  │                                                           │
//! │                  └── ... 其他 16 种资源 ...                                   │
//! │                                                                              │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Decisions
//!
//! 1. **Complete Independence**: Each resource type runs its own 1-8 flow independently.
//!    No waiting for other resources - when one resource finishes init, it immediately
//!    starts its workqueue reconcile loop.
//!
//! 2. **Parallel Initialization**: All 19 resource types initialize in parallel.
//!    Total startup time ≈ time for the slowest single resource (not sum of all).
//!
//! 3. **Fault Isolation**: One resource failing doesn't affect others.
//!
//! 4. **Progressive Ready**: Each resource marks its cache ready independently,
//!    allowing downstream consumers to start using available data sooner.
//!
//! 5. **Graceful Shutdown**: Handles SIGTERM/SIGINT for clean shutdown.
//!
//! 6. **Leader Election**: Managed externally by lifecycle_kubernetes.rs.
//!    This controller focuses solely on resource watching and synchronization.
//!
//! 7. **Metrics**: Prometheus metrics for reconciliation monitoring.
//!
//! 8. **Workqueue**: Go operator-style deduplication and retry with backoff.
//!
//! 9. **ResourceProcessor**: Each resource has its own Processor implementing
//!    the unified processing logic (filter, parse, save, on_change, etc.).

use anyhow::Result;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::watcher;
use kube::{Client, Resource};
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::namespace::NamespaceWatchMode;
use super::resource_controller::{RelinkReason, RelinkSignalSender, ResourceControllerBuilder};
use super::resource_processor::{
    BackendTlsPolicyProcessor, EdgionGatewayConfigProcessor, EdgionPluginsProcessor,
    EdgionStreamPluginsProcessor, EdgionTlsProcessor, EndpointSliceProcessor, EndpointsProcessor,
    GatewayClassProcessor, GatewayProcessor, GrpcRouteProcessor, HttpRouteProcessor, LinkSysProcessor,
    PluginMetadataProcessor, ProcessConfig, ReferenceGrantProcessor, RequeueRegistry, ResourceProcessor,
    SecretProcessor, ServiceProcessor, TcpRouteProcessor, TlsRouteProcessor, UdpRouteProcessor,
};
use super::shutdown::ShutdownSignal;
use super::status::{KubernetesStatusStore, StatusStore};
use crate::core::conf_mgr::MetadataFilterConfig;
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::types::prelude_resources::*;

/// Context for spawn functions
struct SpawnContext {
    watcher_config: watcher::Config,
    shutdown: ShutdownSignal,
    relink_tx: RelinkSignalSender,
    requeue_registry: Arc<RequeueRegistry>,
    process_config: ProcessConfig,
}

/// Spawn a namespaced ResourceController with the given processor
fn spawn<K, P>(
    controller: &KubernetesController,
    processor: P,
    ctx: &SpawnContext,
) -> JoinHandle<Result<()>>
where
    K: Resource<Scope = kube::core::NamespaceResourceScope>
        + Clone
        + Send
        + Sync
        + Debug
        + DeserializeOwned
        + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    P: ResourceProcessor<K> + 'static,
{
    let rc = ResourceControllerBuilder::<K>::new(processor.kind())
        .namespaced(controller.watch_mode.clone())
        .with_processor(processor)
        .with_process_config(ctx.process_config.clone())
        .with_requeue_registry(ctx.requeue_registry.clone())
        .with_shutdown(ctx.shutdown.clone())
        .with_relink_signal(ctx.relink_tx.clone())
        .build(
            controller.client.clone(),
            controller.config_server.clone(),
            ctx.watcher_config.clone(),
        );

    tokio::spawn(async move { rc.run_namespaced().await })
}

/// Spawn a cluster-scoped ResourceController with the given processor
fn spawn_cluster<K, P>(
    controller: &KubernetesController,
    processor: P,
    ctx: &SpawnContext,
) -> JoinHandle<Result<()>>
where
    K: Resource<Scope = kube::core::ClusterResourceScope>
        + Clone
        + Send
        + Sync
        + Debug
        + DeserializeOwned
        + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    P: ResourceProcessor<K> + 'static,
{
    let rc = ResourceControllerBuilder::<K>::new(processor.kind())
        .cluster_scoped()
        .with_processor(processor)
        .with_process_config(ctx.process_config.clone())
        .with_requeue_registry(ctx.requeue_registry.clone())
        .with_shutdown(ctx.shutdown.clone())
        .with_relink_signal(ctx.relink_tx.clone())
        .build(
            controller.client.clone(),
            controller.config_server.clone(),
            ctx.watcher_config.clone(),
        );

    tokio::spawn(async move { rc.run_cluster_scoped().await })
}

/// Kubernetes Controller that spawns independent ResourceControllers for each resource type
///
/// Note: Leader election is handled externally by lifecycle_kubernetes.rs.
/// This controller focuses solely on resource watching and synchronization.
pub struct KubernetesController {
    client: Client,
    config_server: Arc<ConfigServer>,
    #[allow(dead_code)]
    status_store: Arc<dyn StatusStore>,
    gateway_class_name: String,
    watch_mode: NamespaceWatchMode,
    label_selector: Option<String>,
    /// Optional metadata filter configuration for reducing resource memory usage
    metadata_filter: Option<MetadataFilterConfig>,
}

impl KubernetesController {
    /// Create a new KubernetesController
    ///
    /// Note: Prefer using `with_metadata_filter` with an existing Client
    /// to avoid creating multiple Client instances.
    pub async fn new(
        config_server: Arc<ConfigServer>,
        gateway_class_name: String,
        watch_namespaces: Vec<String>,
        label_selector: Option<String>,
    ) -> Result<Self> {
        let client = Client::try_default().await?;
        Self::with_metadata_filter(
            client,
            config_server,
            gateway_class_name,
            watch_namespaces,
            label_selector,
            MetadataFilterConfig::default(),
        )
    }

    /// Create a new KubernetesController with metadata filter
    ///
    /// Accepts an external Client to enable Client reuse across components.
    pub fn with_metadata_filter(
        client: Client,
        config_server: Arc<ConfigServer>,
        gateway_class_name: String,
        watch_namespaces: Vec<String>,
        label_selector: Option<String>,
        metadata_filter: MetadataFilterConfig,
    ) -> Result<Self> {
        let status_store: Arc<dyn StatusStore> =
            Arc::new(KubernetesStatusStore::new(client.clone(), "edgion-controller".to_string()));

        let watch_mode = NamespaceWatchMode::from_namespaces(watch_namespaces);

        tracing::info!(
            component = "k8s_controller",
            watch_mode = ?watch_mode,
            label_selector = ?label_selector,
            gateway_class_name = %gateway_class_name,
            metadata_filter_enabled = true,
            "Creating Kubernetes controller"
        );

        Ok(Self {
            client,
            config_server,
            status_store,
            gateway_class_name,
            watch_mode,
            label_selector,
            metadata_filter: Some(metadata_filter),
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
    /// - Creates its own reflector store
    /// - Waits only for its own store to be ready
    /// - Applies InitAdd for its resources
    /// - Marks its cache ready
    /// - Immediately starts workqueue reconcile loop (no waiting for other resources)
    ///
    /// Also handles:
    /// - Graceful shutdown via provided ShutdownSignal
    /// - 410 Gone detection and relink signaling
    ///
    /// Note: Leader election is handled by the caller (lifecycle_kubernetes.rs).
    /// This method returns after controllers stop. If relink is needed,
    /// the caller should call this again after resetting state.
    pub async fn run(&self, shutdown_signal: ShutdownSignal) -> Result<ControllerExitReason> {
        self.run_controllers(shutdown_signal).await
    }

    /// Internal method to run all controllers
    /// Returns when shutdown is triggered or a relink signal is received
    #[allow(clippy::vec_init_then_push)]
    async fn run_controllers(&self, mut shutdown_signal: ShutdownSignal) -> Result<ControllerExitReason> {
        tracing::info!(
            component = "k8s_controller",
            "Starting Kubernetes controller - spawning 19 independent ResourceControllers"
        );

        // Create relink signal channel
        let (relink_tx, mut relink_rx) = mpsc::channel::<RelinkReason>(10);

        // Create global RequeueRegistry (shared by all ResourceControllers)
        let requeue_registry = Arc::new(RequeueRegistry::new());

        // Create SpawnContext
        let ctx = SpawnContext {
            watcher_config: self.watcher_config(),
            shutdown: shutdown_signal.clone(),
            relink_tx: relink_tx.clone(),
            requeue_registry: requeue_registry.clone(),
            process_config: ProcessConfig {
                metadata_filter: self.metadata_filter.clone(),
            },
        };

        let mut h = Vec::new();

        // ==================== Namespaced Resources (17) ====================
        // Route resources
        h.push(spawn::<HTTPRoute, _>(self, HttpRouteProcessor::new(), &ctx));
        h.push(spawn::<GRPCRoute, _>(self, GrpcRouteProcessor::new(), &ctx));
        h.push(spawn::<TCPRoute, _>(self, TcpRouteProcessor::new(), &ctx));
        h.push(spawn::<UDPRoute, _>(self, UdpRouteProcessor::new(), &ctx));
        h.push(spawn::<TLSRoute, _>(self, TlsRouteProcessor::new(), &ctx));

        // Backend resources
        h.push(spawn::<Service, _>(self, ServiceProcessor::new(), &ctx));
        h.push(spawn::<Endpoints, _>(self, EndpointsProcessor::new(), &ctx));
        h.push(spawn::<EndpointSlice, _>(self, EndpointSliceProcessor::new(), &ctx));

        // TLS related (special processing)
        h.push(spawn::<Secret, _>(self, SecretProcessor::new(), &ctx));
        h.push(spawn::<EdgionTls, _>(self, EdgionTlsProcessor::new(), &ctx));
        h.push(spawn::<BackendTLSPolicy, _>(self, BackendTlsPolicyProcessor::new(), &ctx));

        // Gateway (special processing: filter by gateway_class)
        h.push(spawn::<Gateway, _>(
            self,
            GatewayProcessor::new(self.gateway_class_name.clone()),
            &ctx,
        ));

        // Other namespaced resources
        h.push(spawn::<ReferenceGrant, _>(self, ReferenceGrantProcessor::new(), &ctx));
        h.push(spawn::<EdgionPlugins, _>(self, EdgionPluginsProcessor::new(), &ctx));
        h.push(spawn::<EdgionStreamPlugins, _>(self, EdgionStreamPluginsProcessor::new(), &ctx));
        h.push(spawn::<PluginMetaData, _>(self, PluginMetadataProcessor::new(), &ctx));
        h.push(spawn::<LinkSys, _>(self, LinkSysProcessor::new(), &ctx));

        // ==================== Cluster-Scoped Resources (2) ====================
        h.push(spawn_cluster::<GatewayClass, _>(self, GatewayClassProcessor::new(), &ctx));
        h.push(spawn_cluster::<EdgionGatewayConfig, _>(
            self,
            EdgionGatewayConfigProcessor::new(),
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
