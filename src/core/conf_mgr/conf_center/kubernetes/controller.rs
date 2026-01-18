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
//! 6. **Leader Election**: Optional leader election for HA deployments.
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

use super::leader_election::{LeaderElection, LeaderElectionConfig};
use super::namespace::NamespaceWatchMode;
use super::resource_controller::ResourceControllerBuilder;
use super::shutdown::{ShutdownHandle, ShutdownSignal};
use super::status::{KubernetesStatusStore, StatusStore};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::*;

/// Macro to spawn a standard namespaced ResourceController
/// Usage: spawn_namespaced!(self, handles, watcher_config, shutdown_signal, Type, "Kind", cache_field)
macro_rules! spawn_namespaced {
    ($self:ident, $handles:ident, $watcher_config:ident, $shutdown:ident, $type:ty, $kind:literal, $cache:ident) => {{
        let rc = ResourceControllerBuilder::<$type>::new($kind)
            .namespaced($self.watch_mode.clone())
            .apply_with(|cs, change, r| cs.$cache.apply_change(change, r))
            .with_shutdown($shutdown.clone())
            .build(
                $self.client.clone(),
                $self.config_server.clone(),
                $watcher_config.clone(),
            );
        $handles.push(tokio::spawn(async move { rc.run_namespaced().await }));
    }};
}

/// Macro to spawn a namespaced ResourceController with custom apply function
/// Usage: spawn_namespaced_custom!(self, handles, watcher_config, shutdown_signal, Type, "Kind", apply_fn)
macro_rules! spawn_namespaced_custom {
    ($self:ident, $handles:ident, $watcher_config:ident, $shutdown:ident, $type:ty, $kind:literal, $apply:expr) => {{
        let rc = ResourceControllerBuilder::<$type>::new($kind)
            .namespaced($self.watch_mode.clone())
            .apply_with($apply)
            .with_shutdown($shutdown.clone())
            .build(
                $self.client.clone(),
                $self.config_server.clone(),
                $watcher_config.clone(),
            );
        $handles.push(tokio::spawn(async move { rc.run_namespaced().await }));
    }};
}

/// Macro to spawn a cluster-scoped ResourceController
/// Usage: spawn_cluster!(self, handles, watcher_config, shutdown_signal, Type, "Kind", cache_field)
macro_rules! spawn_cluster {
    ($self:ident, $handles:ident, $watcher_config:ident, $shutdown:ident, $type:ty, $kind:literal, $cache:ident) => {{
        let rc = ResourceControllerBuilder::<$type>::new($kind)
            .cluster_scoped()
            .apply_with(|cs, change, r| cs.$cache.apply_change(change, r))
            .with_shutdown($shutdown.clone())
            .build(
                $self.client.clone(),
                $self.config_server.clone(),
                $watcher_config.clone(),
            );
        $handles.push(tokio::spawn(async move { rc.run_cluster_scoped().await }));
    }};
}

/// Leader election mode
#[derive(Clone, Debug, Default)]
pub enum LeaderElectionMode {
    /// No leader election - single instance mode
    #[default]
    Disabled,
    /// Leader election enabled with configuration
    Enabled(LeaderElectionConfig),
}

/// Kubernetes Controller that spawns independent ResourceControllers for each resource type
pub struct KubernetesController {
    client: Client,
    config_server: Arc<ConfigServer>,
    #[allow(dead_code)]
    status_store: Arc<dyn StatusStore>,
    gateway_class_name: String,
    watch_mode: NamespaceWatchMode,
    label_selector: Option<String>,
    leader_election_mode: LeaderElectionMode,
}

impl KubernetesController {
    /// Create a new KubernetesController
    pub async fn new(
        config_server: Arc<ConfigServer>,
        gateway_class_name: String,
        watch_namespaces: Vec<String>,
        label_selector: Option<String>,
    ) -> Result<Self> {
        Self::with_leader_election(
            config_server,
            gateway_class_name,
            watch_namespaces,
            label_selector,
            LeaderElectionMode::Disabled,
        )
        .await
    }

    /// Create a new KubernetesController with leader election
    pub async fn with_leader_election(
        config_server: Arc<ConfigServer>,
        gateway_class_name: String,
        watch_namespaces: Vec<String>,
        label_selector: Option<String>,
        leader_election_mode: LeaderElectionMode,
    ) -> Result<Self> {
        let client = Client::try_default().await?;
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
            leader_election = ?leader_election_mode,
            "Creating Kubernetes controller with independent ResourceControllers"
        );

        Ok(Self {
            client,
            config_server,
            status_store,
            gateway_class_name,
            watch_mode,
            label_selector,
            leader_election_mode,
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
    /// - Graceful shutdown on SIGTERM/SIGINT
    /// - Optional leader election for HA deployments
    pub async fn run(&self) -> Result<()> {
        // Setup shutdown handling
        let shutdown_handle = ShutdownHandle::new();
        let shutdown_signal = shutdown_handle.signal();

        // Spawn signal handler
        let signal_handle = shutdown_handle.clone();
        tokio::spawn(async move {
            signal_handle.wait_for_signals().await;
        });

        // Handle leader election if enabled
        match &self.leader_election_mode {
            LeaderElectionMode::Disabled => {
                tracing::info!(
                    component = "k8s_controller",
                    "Leader election disabled - running as single instance"
                );
                self.run_controllers(shutdown_signal).await
            }
            LeaderElectionMode::Enabled(config) => {
                tracing::info!(
                    component = "k8s_controller",
                    lease_name = %config.lease_name,
                    lease_namespace = %config.lease_namespace,
                    identity = %config.identity,
                    "Leader election enabled - waiting for leadership"
                );

                let leader_election = LeaderElection::new(self.client.clone(), config.clone());
                let leader_handle = leader_election.handle();

                // Spawn leader election loop
                let le = leader_election.clone();
                tokio::spawn(async move {
                    if let Err(e) = le.run().await {
                        tracing::error!(error = %e, "Leader election failed");
                    }
                });

                // Wait until we become leader (with shutdown support)
                if !leader_handle
                    .wait_until_leader_with_shutdown(shutdown_signal.clone())
                    .await
                {
                    tracing::info!(
                        component = "k8s_controller",
                        "Shutdown requested before acquiring leadership"
                    );
                    return Ok(());
                }
                tracing::info!(
                    component = "k8s_controller",
                    "Acquired leadership, starting controllers"
                );

                // Monitor leadership loss and trigger shutdown if we lose it
                let leader_handle_monitor = leader_handle.clone();
                let shutdown_for_leader = shutdown_handle.clone();
                tokio::spawn(async move {
                    // Poll for leadership loss
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        if !leader_handle_monitor.is_leader() {
                            tracing::warn!(
                                component = "k8s_controller",
                                "Lost leadership - triggering shutdown of all controllers"
                            );
                            shutdown_for_leader.shutdown();
                            break;
                        }
                    }
                });

                self.run_controllers(shutdown_signal).await
            }
        }
    }

    /// Internal method to run all controllers
    async fn run_controllers(&self, shutdown_signal: ShutdownSignal) -> Result<()> {
        tracing::info!(
            component = "k8s_controller",
            "Starting Kubernetes controller - spawning 19 independent ResourceControllers"
        );

        let watcher_config = self.watcher_config();
        let mut handles = Vec::new();

        // ==================== Standard Namespaced Resources (14) ====================
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, HTTPRoute, "HTTPRoute", routes);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, GRPCRoute, "GRPCRoute", grpc_routes);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, TCPRoute, "TCPRoute", tcp_routes);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, UDPRoute, "UDPRoute", udp_routes);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, TLSRoute, "TLSRoute", tls_routes);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, Service, "Service", services);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, Endpoints, "Endpoints", endpoints);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, EndpointSlice, "EndpointSlice", endpoint_slices);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, ReferenceGrant, "ReferenceGrant", reference_grants);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, EdgionPlugins, "EdgionPlugins", edgion_plugins);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, EdgionStreamPlugins, "EdgionStreamPlugins", edgion_stream_plugins);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, BackendTLSPolicy, "BackendTLSPolicy", backend_tls_policies);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, PluginMetaData, "PluginMetaData", plugin_metadata);
        spawn_namespaced!(self, handles, watcher_config, shutdown_signal, LinkSys, "LinkSys", link_sys);

        // ==================== Namespaced with Custom Apply (2) ====================
        spawn_namespaced_custom!(self, handles, watcher_config, shutdown_signal, Secret, "Secret",
            |cs, change, r| cs.apply_secret_change(change, r));

        // EdgionTls - standard apply (watches removed, handled by apply logic)
        spawn_namespaced_custom!(self, handles, watcher_config, shutdown_signal, EdgionTls, "EdgionTls",
            |cs, change, r| cs.apply_edgion_tls_change(change, r));

        // ==================== Gateway (with filter) ====================
        {
            let gateway_class = self.gateway_class_name.clone();
            let shutdown = shutdown_signal.clone();
            let rc = ResourceControllerBuilder::<Gateway>::new("Gateway")
                .namespaced(self.watch_mode.clone())
                .filter(move |g| g.spec.gateway_class_name == gateway_class)
                .apply_with(|cs, change, r| cs.apply_gateway_change(change, r))
                .with_shutdown(shutdown)
                .build(
                    self.client.clone(),
                    self.config_server.clone(),
                    watcher_config.clone(),
                );
            handles.push(tokio::spawn(async move { rc.run_namespaced().await }));
        }

        // ==================== Cluster-Scoped Resources (2) ====================
        spawn_cluster!(self, handles, watcher_config, shutdown_signal, GatewayClass, "GatewayClass", gateway_classes);
        spawn_cluster!(self, handles, watcher_config, shutdown_signal, EdgionGatewayConfig, "EdgionGatewayConfig", edgion_gateway_configs);

        tracing::info!(
            component = "k8s_controller",
            count = handles.len(),
            "All ResourceControllers spawned - each running independently"
        );

        // Wait for all controllers (they run until shutdown or program exit)
        futures::future::join_all(handles).await;

        tracing::warn!(component = "k8s_controller", "All controllers have stopped");
        Ok(())
    }
}
