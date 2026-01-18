//! Kubernetes Controller using kube-runtime
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
//! │                  │       [1-8 独立流程: create→reflector→wait→init→reconcile]│
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
//!    starts its reconcile loop.
//!
//! 2. **Parallel Initialization**: All 19 resource types initialize in parallel.
//!    Total startup time ≈ time for the slowest single resource (not sum of all).
//!
//! 3. **Fault Isolation**: One resource failing doesn't affect others.
//!
//! 4. **Progressive Ready**: Each resource marks its cache ready independently,
//!    allowing downstream consumers to start using available data sooner.

use anyhow::Result;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::watcher;
use kube::Client;
use std::sync::Arc;

use super::namespace::NamespaceWatchMode;
use super::reconcilers::*;
use super::resource_controller::ResourceControllerBuilder;
use super::status::{KubernetesStatusStore, StatusStore};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::*;

/// Macro to spawn a standard namespaced ResourceController
/// Usage: spawn_namespaced!(Type, "Kind", cache_field, reconcile_fn)
macro_rules! spawn_namespaced {
    ($self:ident, $handles:ident, $watcher_config:ident, $type:ty, $kind:literal, $cache:ident, $reconcile:ident) => {{
        let rc = ResourceControllerBuilder::<$type>::new($kind)
            .namespaced($self.watch_mode.clone())
            .apply_with(|cs, change, r| cs.$cache.apply_change(change, r))
            .build(
                $self.client.clone(),
                $self.config_server.clone(),
                $self.status_store.clone(),
                $self.gateway_class_name.clone(),
                $watcher_config.clone(),
                $reconcile,
            );
        $handles.push(tokio::spawn(async move { rc.run_namespaced().await }));
    }};
}

/// Macro to spawn a namespaced ResourceController with custom apply function
/// Usage: spawn_namespaced_custom!(Type, "Kind", |cs, change, r| cs.apply_xxx(change, r), reconcile_fn)
macro_rules! spawn_namespaced_custom {
    ($self:ident, $handles:ident, $watcher_config:ident, $type:ty, $kind:literal, $apply:expr, $reconcile:ident) => {{
        let rc = ResourceControllerBuilder::<$type>::new($kind)
            .namespaced($self.watch_mode.clone())
            .apply_with($apply)
            .build(
                $self.client.clone(),
                $self.config_server.clone(),
                $self.status_store.clone(),
                $self.gateway_class_name.clone(),
                $watcher_config.clone(),
                $reconcile,
            );
        $handles.push(tokio::spawn(async move { rc.run_namespaced().await }));
    }};
}

/// Macro to spawn a cluster-scoped ResourceController
/// Usage: spawn_cluster!(Type, "Kind", cache_field, reconcile_fn)
macro_rules! spawn_cluster {
    ($self:ident, $handles:ident, $watcher_config:ident, $type:ty, $kind:literal, $cache:ident, $reconcile:ident) => {{
        let rc = ResourceControllerBuilder::<$type>::new($kind)
            .cluster_scoped()
            .apply_with(|cs, change, r| cs.$cache.apply_change(change, r))
            .build(
                $self.client.clone(),
                $self.config_server.clone(),
                $self.status_store.clone(),
                $self.gateway_class_name.clone(),
                $watcher_config.clone(),
                $reconcile,
            );
        $handles.push(tokio::spawn(async move { rc.run_cluster_scoped().await }));
    }};
}

/// Kubernetes Controller that spawns independent ResourceControllers for each resource type
pub struct KubernetesController {
    client: Client,
    config_server: Arc<ConfigServer>,
    status_store: Arc<dyn StatusStore>,
    gateway_class_name: String,
    watch_mode: NamespaceWatchMode,
    label_selector: Option<String>,
}

impl KubernetesController {
    /// Create a new KubernetesController
    pub async fn new(
        config_server: Arc<ConfigServer>,
        gateway_class_name: String,
        watch_namespaces: Vec<String>,
        label_selector: Option<String>,
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
            "Creating Kubernetes controller with independent ResourceControllers"
        );

        Ok(Self {
            client,
            config_server,
            status_store,
            gateway_class_name,
            watch_mode,
            label_selector,
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
    /// - Immediately starts reconcile loop (no waiting for other resources)
    pub async fn run(&self) -> Result<()> {
        tracing::info!(
            component = "k8s_controller",
            "Starting Kubernetes controller - spawning 19 independent ResourceControllers"
        );

        let watcher_config = self.watcher_config();
        let mut handles = Vec::new();

        // ==================== Standard Namespaced Resources (14) ====================
        spawn_namespaced!(self, handles, watcher_config, HTTPRoute, "HTTPRoute", routes, reconcile_http_route);
        spawn_namespaced!(self, handles, watcher_config, GRPCRoute, "GRPCRoute", grpc_routes, reconcile_grpc_route);
        spawn_namespaced!(self, handles, watcher_config, TCPRoute, "TCPRoute", tcp_routes, reconcile_tcp_route);
        spawn_namespaced!(self, handles, watcher_config, UDPRoute, "UDPRoute", udp_routes, reconcile_udp_route);
        spawn_namespaced!(self, handles, watcher_config, TLSRoute, "TLSRoute", tls_routes, reconcile_tls_route);
        spawn_namespaced!(self, handles, watcher_config, Service, "Service", services, reconcile_service);
        spawn_namespaced!(self, handles, watcher_config, Endpoints, "Endpoints", endpoints, reconcile_endpoints);
        spawn_namespaced!(self, handles, watcher_config, EndpointSlice, "EndpointSlice", endpoint_slices, reconcile_endpoint_slice);
        spawn_namespaced!(self, handles, watcher_config, ReferenceGrant, "ReferenceGrant", reference_grants, reconcile_reference_grant);
        spawn_namespaced!(self, handles, watcher_config, EdgionPlugins, "EdgionPlugins", edgion_plugins, reconcile_edgion_plugins);
        spawn_namespaced!(self, handles, watcher_config, EdgionStreamPlugins, "EdgionStreamPlugins", edgion_stream_plugins, reconcile_edgion_stream_plugins);
        spawn_namespaced!(self, handles, watcher_config, BackendTLSPolicy, "BackendTLSPolicy", backend_tls_policies, reconcile_backend_tls_policy);
        spawn_namespaced!(self, handles, watcher_config, PluginMetaData, "PluginMetaData", plugin_metadata, reconcile_plugin_metadata);
        spawn_namespaced!(self, handles, watcher_config, LinkSys, "LinkSys", link_sys, reconcile_link_sys);

        // ==================== Namespaced with Custom Apply (2) ====================
        spawn_namespaced_custom!(self, handles, watcher_config, Secret, "Secret",
            |cs, change, r| cs.apply_secret_change(change, r), reconcile_secret);
        spawn_namespaced_custom!(self, handles, watcher_config, EdgionTls, "EdgionTls",
            |cs, change, r| cs.apply_edgion_tls_change(change, r), reconcile_edgion_tls);

        // ==================== Gateway (with filter) ====================
        {
            let gateway_class = self.gateway_class_name.clone();
            let rc = ResourceControllerBuilder::<Gateway>::new("Gateway")
                .namespaced(self.watch_mode.clone())
                .filter(move |g| g.spec.gateway_class_name == gateway_class)
                .apply_with(|cs, change, r| cs.apply_gateway_change(change, r))
                .build(
                    self.client.clone(),
                    self.config_server.clone(),
                    self.status_store.clone(),
                    self.gateway_class_name.clone(),
                    watcher_config.clone(),
                    reconcile_gateway,
                );
            handles.push(tokio::spawn(async move { rc.run_namespaced().await }));
        }

        // ==================== Cluster-Scoped Resources (2) ====================
        spawn_cluster!(self, handles, watcher_config, GatewayClass, "GatewayClass", gateway_classes, reconcile_gateway_class);
        spawn_cluster!(self, handles, watcher_config, EdgionGatewayConfig, "EdgionGatewayConfig", edgion_gateway_configs, reconcile_edgion_gateway_config);

        tracing::info!(
            component = "k8s_controller",
            count = handles.len(),
            "All ResourceControllers spawned - each running independently"
        );

        // Wait for all controllers (they run until program exit)
        futures::future::join_all(handles).await;

        tracing::warn!(component = "k8s_controller", "All controllers have stopped");
        Ok(())
    }
}
