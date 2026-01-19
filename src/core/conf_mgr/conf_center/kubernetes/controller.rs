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

use anyhow::Result;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::watcher;
use kube::Client;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::namespace::NamespaceWatchMode;
use super::resource_controller::{RelinkReason, ResourceControllerBuilder};
use super::shutdown::ShutdownSignal;
use super::status::{KubernetesStatusStore, StatusStore};
use crate::core::conf_mgr::MetadataFilterConfig;
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::CacheEventDispatch;
use crate::core::utils::clean_metadata;
use crate::types::prelude_resources::*;

/// Macro to spawn a standard namespaced ResourceController
/// Usage: spawn_namespaced!(self, handles, watcher_config, shutdown_signal, relink_tx, Type, "Kind", cache_field)
macro_rules! spawn_namespaced {
    ($self:ident, $handles:ident, $watcher_config:ident, $shutdown:ident, $relink_tx:ident, $type:ty, $kind:literal, $cache:ident) => {{
        let filter_config = $self.metadata_filter.clone();
        let rc = ResourceControllerBuilder::<$type>::new($kind)
            .namespaced($self.watch_mode.clone())
            .apply_with(move |cs, change, mut r| {
                if let Some(ref config) = filter_config {
                    clean_metadata(&mut r, config);
                }
                cs.$cache.apply_change(change, r)
            })
            .with_shutdown($shutdown.clone())
            .with_relink_signal($relink_tx.clone())
            .build(
                $self.client.clone(),
                $self.config_server.clone(),
                $watcher_config.clone(),
            );
        $handles.push(tokio::spawn(async move { rc.run_namespaced().await }));
    }};
}

/// Macro to spawn a namespaced ResourceController with custom apply function
/// Usage: spawn_namespaced_custom!(self, handles, watcher_config, shutdown_signal, relink_tx, Type, "Kind", apply_fn)
macro_rules! spawn_namespaced_custom {
    ($self:ident, $handles:ident, $watcher_config:ident, $shutdown:ident, $relink_tx:ident, $type:ty, $kind:literal, $apply:expr) => {{
        let filter_config = $self.metadata_filter.clone();
        let apply_fn: fn(&ConfigServer, crate::core::conf_sync::traits::ResourceChange, $type) = $apply;
        let rc = ResourceControllerBuilder::<$type>::new($kind)
            .namespaced($self.watch_mode.clone())
            .apply_with(move |cs, change, mut r| {
                if let Some(ref config) = filter_config {
                    clean_metadata(&mut r, config);
                }
                apply_fn(cs, change, r)
            })
            .with_shutdown($shutdown.clone())
            .with_relink_signal($relink_tx.clone())
            .build(
                $self.client.clone(),
                $self.config_server.clone(),
                $watcher_config.clone(),
            );
        $handles.push(tokio::spawn(async move { rc.run_namespaced().await }));
    }};
}

/// Macro to spawn a cluster-scoped ResourceController
/// Usage: spawn_cluster!(self, handles, watcher_config, shutdown_signal, relink_tx, Type, "Kind", cache_field)
macro_rules! spawn_cluster {
    ($self:ident, $handles:ident, $watcher_config:ident, $shutdown:ident, $relink_tx:ident, $type:ty, $kind:literal, $cache:ident) => {{
        let filter_config = $self.metadata_filter.clone();
        let rc = ResourceControllerBuilder::<$type>::new($kind)
            .cluster_scoped()
            .apply_with(move |cs, change, mut r| {
                if let Some(ref config) = filter_config {
                    clean_metadata(&mut r, config);
                }
                cs.$cache.apply_change(change, r)
            })
            .with_shutdown($shutdown.clone())
            .with_relink_signal($relink_tx.clone())
            .build(
                $self.client.clone(),
                $self.config_server.clone(),
                $watcher_config.clone(),
            );
        $handles.push(tokio::spawn(async move { rc.run_cluster_scoped().await }));
    }};
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
        let status_store: Arc<dyn StatusStore> = Arc::new(KubernetesStatusStore::new(
            client.clone(),
            "edgion-controller".to_string(),
        ));

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
    async fn run_controllers(&self, mut shutdown_signal: ShutdownSignal) -> Result<ControllerExitReason> {
        tracing::info!(
            component = "k8s_controller",
            "Starting Kubernetes controller - spawning 19 independent ResourceControllers"
        );

        let watcher_config = self.watcher_config();
        let mut handles = Vec::new();

        // Create relink signal channel
        // Any ResourceController can send a signal when 410 Gone is detected
        let (relink_tx, mut relink_rx) = mpsc::channel::<RelinkReason>(10);

        // ==================== Standard Namespaced Resources (14) ====================
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            HTTPRoute,
            "HTTPRoute",
            routes
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            GRPCRoute,
            "GRPCRoute",
            grpc_routes
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            TCPRoute,
            "TCPRoute",
            tcp_routes
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            UDPRoute,
            "UDPRoute",
            udp_routes
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            TLSRoute,
            "TLSRoute",
            tls_routes
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            Service,
            "Service",
            services
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            Endpoints,
            "Endpoints",
            endpoints
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            EndpointSlice,
            "EndpointSlice",
            endpoint_slices
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            ReferenceGrant,
            "ReferenceGrant",
            reference_grants
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            EdgionPlugins,
            "EdgionPlugins",
            edgion_plugins
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            EdgionStreamPlugins,
            "EdgionStreamPlugins",
            edgion_stream_plugins
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            BackendTLSPolicy,
            "BackendTLSPolicy",
            backend_tls_policies
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            PluginMetaData,
            "PluginMetaData",
            plugin_metadata
        );
        spawn_namespaced!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            LinkSys,
            "LinkSys",
            link_sys
        );

        // ==================== Namespaced with Custom Apply (2) ====================
        spawn_namespaced_custom!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            Secret,
            "Secret",
            |cs, change, r| cs.apply_secret_change(change, r)
        );

        // EdgionTls - standard apply (watches removed, handled by apply logic)
        spawn_namespaced_custom!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            EdgionTls,
            "EdgionTls",
            |cs, change, r| cs.apply_edgion_tls_change(change, r)
        );

        // ==================== Gateway (with filter) ====================
        {
            let gateway_class = self.gateway_class_name.clone();
            let shutdown = shutdown_signal.clone();
            let relink = relink_tx.clone();
            let filter_config = self.metadata_filter.clone();
            let rc = ResourceControllerBuilder::<Gateway>::new("Gateway")
                .namespaced(self.watch_mode.clone())
                .filter(move |g| g.spec.gateway_class_name == gateway_class)
                .apply_with(move |cs, change, mut r| {
                    if let Some(ref config) = filter_config {
                        clean_metadata(&mut r, config);
                    }
                    cs.apply_gateway_change(change, r)
                })
                .with_shutdown(shutdown)
                .with_relink_signal(relink)
                .build(self.client.clone(), self.config_server.clone(), watcher_config.clone());
            handles.push(tokio::spawn(async move { rc.run_namespaced().await }));
        }

        // ==================== Cluster-Scoped Resources (2) ====================
        spawn_cluster!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            GatewayClass,
            "GatewayClass",
            gateway_classes
        );
        spawn_cluster!(
            self,
            handles,
            watcher_config,
            shutdown_signal,
            relink_tx,
            EdgionGatewayConfig,
            "EdgionGatewayConfig",
            edgion_gateway_configs
        );

        // Drop our copy of the sender so we can detect when all controllers stop
        drop(relink_tx);

        tracing::info!(
            component = "k8s_controller",
            count = handles.len(),
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
            _ = futures::future::join_all(&mut handles) => {
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
