//! Kubernetes Controller
//!
//! Watches Kubernetes resources and keeps the KubernetesStore cache up to date
//!
//! Supports three namespace watch modes:
//! - All namespaces (default): watch_namespaces = []
//! - Single namespace: watch_namespaces = ["ns1"]
//! - Multiple namespaces: watch_namespaces = ["ns1", "ns2"]
//!
//! Also supports label selector filtering for all watched resources.
//!
//! Status updates are event-driven: when Gateway or HTTPRoute changes,
//! the controller immediately updates the K8s Status subresource.

use anyhow::Result;
use futures::TryStreamExt;
use kube::core::NamespaceResourceScope;
use kube::runtime::watcher;
use kube::{Api, Client, ResourceExt};
use std::sync::Arc;

use super::{KubernetesStore, StatusReconciler};
use crate::core::conf_mgr::conf_center::{KubernetesStatusStore, StatusStore};
use crate::core::conf_mgr::resource_check::{self, check_edgion_tls, ResourceCheckContext};
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{CacheEventDispatch, ConfigServer};
use crate::core::observe::metrics::global_metrics;
use crate::types::prelude_resources::*;

use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

/// Namespace watch mode for the controller
#[derive(Debug, Clone)]
pub enum NamespaceWatchMode {
    /// Watch all namespaces (cluster-wide)
    AllNamespaces,
    /// Watch a single namespace
    SingleNamespace(String),
    /// Watch multiple specific namespaces
    MultipleNamespaces(Vec<String>),
}

impl NamespaceWatchMode {
    /// Create from a list of namespaces
    pub fn from_namespaces(namespaces: Vec<String>) -> Self {
        match namespaces.len() {
            0 => Self::AllNamespaces,
            1 => Self::SingleNamespace(namespaces.into_iter().next().unwrap()),
            _ => Self::MultipleNamespaces(namespaces),
        }
    }

    /// Check if watching all namespaces
    #[allow(dead_code)]
    pub fn is_all_namespaces(&self) -> bool {
        matches!(self, Self::AllNamespaces)
    }
}

/// Kubernetes Controller that watches resources and updates ConfigServer
pub struct KubernetesController {
    client: Client,
    config_server: Arc<ConfigServer>,
    store: Arc<KubernetesStore>,
    #[allow(dead_code)]
    status_store: Arc<dyn StatusStore>,
    #[allow(dead_code)]
    reconciler: StatusReconciler,
    gateway_class_name: String,
    /// Namespace watch mode
    watch_mode: NamespaceWatchMode,
    /// Optional label selector for filtering resources
    label_selector: Option<String>,
}

impl KubernetesController {
    /// Create a new KubernetesController
    ///
    /// # Arguments
    /// * `config_server` - The configuration server to update
    /// * `store` - The Kubernetes store for caching resources
    /// * `gateway_class_name` - The gateway class name to watch
    /// * `watch_namespaces` - List of namespaces to watch (empty = all)
    /// * `label_selector` - Optional label selector for filtering resources
    pub async fn new(
        config_server: Arc<ConfigServer>,
        store: Arc<KubernetesStore>,
        gateway_class_name: String,
        watch_namespaces: Vec<String>,
        label_selector: Option<String>,
    ) -> Result<Self> {
        let client = Client::try_default().await?;
        let status_store: Arc<dyn StatusStore> = Arc::new(KubernetesStatusStore::new(
            client.clone(),
            "edgion-controller".to_string(),
        ));
        let reconciler = StatusReconciler::new(config_server.clone(), status_store.clone(), gateway_class_name.clone());

        let watch_mode = NamespaceWatchMode::from_namespaces(watch_namespaces);

        tracing::info!(
            component = "k8s_controller",
            event = "controller_config",
            watch_mode = ?watch_mode,
            label_selector = ?label_selector,
            "Kubernetes controller configured"
        );

        Ok(Self {
            client,
            config_server,
            store,
            status_store,
            reconciler,
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

    /// Run all watchers concurrently
    ///
    /// Status updates are event-driven: Gateway and HTTPRoute watchers
    /// immediately update K8s Status when resources change.
    pub async fn run(&self) -> Result<()> {
        tracing::info!(
            component = "k8s_controller",
            event = "controller_start",
            watch_mode = ?self.watch_mode,
            "Starting Kubernetes controller (event-driven status updates)"
        );

        // Run all watchers concurrently
        // Note: If any watcher fails, all will be cancelled
        //
        // Resource types:
        // - Cluster-scoped: GatewayClass (always watch all)
        // - Namespace-scoped: Everything else (respects watch_mode)
        //
        // Status updates:
        // - Gateway and HTTPRoute: event-driven status updates in their watchers
        // - No polling reconcile loop needed
        tokio::try_join!(
            // Cluster-scoped resources (always watch all namespaces)
            self.watch_gateway_classes(),
            // Namespace-scoped base conf resources (Gateway has event-driven status)
            self.watch_gateways_with_status(),
            self.watch_edgion_gateway_configs(),
            // Route resources (HTTPRoute has event-driven status)
            self.watch_http_routes_with_status(),
            self.watch_grpc_routes(),
            self.watch_tcp_routes(),
            self.watch_udp_routes(),
            self.watch_tls_routes(),
            // Backend resources
            self.watch_services(),
            self.watch_endpoints(),
            self.watch_endpoint_slices(),
            // Other resources (EdgionTls has check before apply)
            self.watch_secrets(),
            self.watch_reference_grants(),
            self.watch_edgion_plugins(),
            self.watch_edgion_stream_plugins(),
            self.watch_edgion_tls_with_check(),
            self.watch_backend_tls_policies(),
            self.watch_plugin_metadata(),
            self.watch_link_sys(),
        )?;

        Ok(())
    }

    /// Run a watcher for namespace-scoped resources based on watch mode
    async fn run_namespaced_watcher<T, F>(&self, kind: &str, handler: F) -> Result<()>
    where
        T: kube::Resource<DynamicType = (), Scope = NamespaceResourceScope>
            + Clone
            + serde::Serialize
            + serde::de::DeserializeOwned
            + std::fmt::Debug
            + Send
            + Sync
            + 'static,
        F: Fn(&ConfigServer, ResourceChange, T) + Clone + Send + Sync + 'static,
    {
        match &self.watch_mode {
            NamespaceWatchMode::AllNamespaces => {
                let api: Api<T> = Api::all(self.client.clone());
                self.run_single_watcher(api, kind, handler).await
            }
            NamespaceWatchMode::SingleNamespace(ns) => {
                let api: Api<T> = Api::namespaced(self.client.clone(), ns);
                self.run_single_watcher(api, kind, handler).await
            }
            NamespaceWatchMode::MultipleNamespaces(namespaces) => {
                // For multiple namespaces, run watchers concurrently
                let futures: Vec<_> = namespaces
                    .iter()
                    .map(|ns| {
                        let api: Api<T> = Api::namespaced(self.client.clone(), ns);
                        let handler = handler.clone();
                        let kind = kind.to_string();
                        let config = self.watcher_config();
                        let store = self.store.clone();
                        let config_server = self.config_server.clone();

                        async move {
                            let watcher = watcher(api, config);
                            tokio::pin!(watcher);
                            while let Some(event) = watcher
                                .try_next()
                                .await
                                .map_err(|e| anyhow::anyhow!("Watcher for {} failed: {}", kind, e))?
                            {
                                Self::handle_event_static(&store, &config_server, event, &kind, &handler).await?;
                            }
                            Ok::<(), anyhow::Error>(())
                        }
                    })
                    .collect();

                futures::future::try_join_all(futures).await?;
                Ok(())
            }
        }
    }

    /// Run a single watcher
    async fn run_single_watcher<T, F>(&self, api: Api<T>, kind: &str, handler: F) -> Result<()>
    where
        T: kube::Resource<DynamicType = ()>
            + Clone
            + serde::Serialize
            + serde::de::DeserializeOwned
            + std::fmt::Debug
            + Send
            + Sync
            + 'static,
        F: Fn(&ConfigServer, ResourceChange, T),
    {
        let config = self.watcher_config();
        let watcher = watcher(api, config);
        tokio::pin!(watcher);
        while let Some(event) = watcher
            .try_next()
            .await
            .map_err(|e| anyhow::anyhow!("Watcher for {} failed: {}", kind, e))?
        {
            self.handle_event(event, kind, &handler).await?;
        }
        Ok(())
    }

    /// Static version of handle_event for use in closures
    async fn handle_event_static<T, F>(
        store: &KubernetesStore,
        config_server: &ConfigServer,
        event: watcher::Event<T>,
        kind: &str,
        handler: &F,
    ) -> Result<()>
    where
        T: kube::Resource + Clone + serde::Serialize,
        <T as kube::Resource>::DynamicType: Default,
        F: Fn(&ConfigServer, ResourceChange, T),
    {
        match event {
            watcher::Event::Apply(resource) => {
                let yaml = serde_yaml::to_string(&resource)?;
                store
                    .apply_resource(
                        kind.to_string(),
                        kube::ResourceExt::namespace(&resource),
                        kube::ResourceExt::name_any(&resource),
                        yaml,
                    )
                    .await;
                handler(config_server, ResourceChange::EventAdd, resource);
            }
            watcher::Event::Delete(resource) => {
                store
                    .remove_resource(
                        kind,
                        kube::ResourceExt::namespace(&resource).as_deref(),
                        &kube::ResourceExt::name_any(&resource),
                    )
                    .await;
                handler(config_server, ResourceChange::EventDelete, resource);
            }
            watcher::Event::Init => {}
            watcher::Event::InitApply(resource) => {
                let yaml = serde_yaml::to_string(&resource)?;
                store
                    .apply_resource(
                        kind.to_string(),
                        kube::ResourceExt::namespace(&resource),
                        kube::ResourceExt::name_any(&resource),
                        yaml,
                    )
                    .await;
                handler(config_server, ResourceChange::InitAdd, resource);
            }
            watcher::Event::InitDone => {
                // Mark this cache as ready
                config_server.set_cache_ready_by_kind(kind);
                tracing::info!("{} watcher initial sync complete, cache marked as ready", kind);
            }
        }
        Ok(())
    }

    // Helper function to handle watch events for most resources
    async fn handle_event<T, F>(&self, event: watcher::Event<T>, kind: &str, handler: F) -> Result<()>
    where
        T: kube::Resource + Clone + serde::Serialize,
        <T as kube::Resource>::DynamicType: Default,
        F: Fn(&ConfigServer, ResourceChange, T),
    {
        Self::handle_event_static(&self.store, &self.config_server, event, kind, &handler).await
    }

    // =========================================================================
    // Namespace-scoped resource watchers
    // These respect the watch_mode configuration
    // =========================================================================

    /// Watch HTTPRoutes with event-driven status updates
    async fn watch_http_routes_with_status(&self) -> Result<()> {
        let kind = "HTTPRoute";
        let status_store = self.status_store.clone();
        let gateway_class_name = self.gateway_class_name.clone();

        match &self.watch_mode {
            NamespaceWatchMode::AllNamespaces => {
                let api: Api<HTTPRoute> = Api::all(self.client.clone());
                self.run_http_route_watcher_with_status(api, kind, status_store, gateway_class_name)
                    .await
            }
            NamespaceWatchMode::SingleNamespace(ns) => {
                let api: Api<HTTPRoute> = Api::namespaced(self.client.clone(), ns);
                self.run_http_route_watcher_with_status(api, kind, status_store, gateway_class_name)
                    .await
            }
            NamespaceWatchMode::MultipleNamespaces(namespaces) => {
                let futures: Vec<_> = namespaces
                    .iter()
                    .map(|ns| {
                        let api: Api<HTTPRoute> = Api::namespaced(self.client.clone(), ns);
                        let kind = kind.to_string();
                        let config = self.watcher_config();
                        let store = self.store.clone();
                        let config_server = self.config_server.clone();
                        let status_store = self.status_store.clone();

                        async move {
                            let watcher = watcher(api, config);
                            tokio::pin!(watcher);
                            while let Some(event) = watcher
                                .try_next()
                                .await
                                .map_err(|e| anyhow::anyhow!("Watcher for {} failed: {}", kind, e))?
                            {
                                Self::handle_http_route_event(&store, &config_server, &status_store, event, &kind).await?;
                            }
                            Ok::<(), anyhow::Error>(())
                        }
                    })
                    .collect();

                futures::future::try_join_all(futures).await?;
                Ok(())
            }
        }
    }

    /// Run HTTPRoute watcher with status updates
    async fn run_http_route_watcher_with_status(
        &self,
        api: Api<HTTPRoute>,
        kind: &str,
        status_store: Arc<dyn StatusStore>,
        _gateway_class_name: String,
    ) -> Result<()> {
        let config = self.watcher_config();
        let watcher = watcher(api, config);
        tokio::pin!(watcher);
        while let Some(event) = watcher
            .try_next()
            .await
            .map_err(|e| anyhow::anyhow!("Watcher for {} failed: {}", kind, e))?
        {
            Self::handle_http_route_event(&self.store, &self.config_server, &status_store, event, kind).await?;
        }
        Ok(())
    }

    /// Handle HTTPRoute event with status update
    async fn handle_http_route_event(
        store: &KubernetesStore,
        config_server: &ConfigServer,
        status_store: &Arc<dyn StatusStore>,
        event: watcher::Event<HTTPRoute>,
        kind: &str,
    ) -> Result<()> {
        match event {
            watcher::Event::Apply(resource) => {
                let yaml = serde_yaml::to_string(&resource)?;
                store
                    .apply_resource(kind.to_string(), resource.namespace(), resource.name_any(), yaml)
                    .await;
                config_server.routes.apply_change(ResourceChange::EventAdd, resource.clone());

                // Event-driven status update with comparison
                Self::update_http_route_status_if_needed(status_store, &resource).await;
            }
            watcher::Event::InitApply(resource) => {
                let yaml = serde_yaml::to_string(&resource)?;
                store
                    .apply_resource(kind.to_string(), resource.namespace(), resource.name_any(), yaml)
                    .await;
                config_server.routes.apply_change(ResourceChange::InitAdd, resource.clone());

                // Event-driven status update with comparison
                Self::update_http_route_status_if_needed(status_store, &resource).await;
            }
            watcher::Event::Delete(resource) => {
                store
                    .remove_resource(kind, resource.namespace().as_deref(), &resource.name_any())
                    .await;
                config_server.routes.apply_change(ResourceChange::EventDelete, resource);
            }
            watcher::Event::Init => {}
            watcher::Event::InitDone => {
                config_server.set_cache_ready_by_kind(kind);
                tracing::info!("{} watcher initial sync complete, cache marked as ready", kind);
            }
        }
        Ok(())
    }

    /// Update HTTPRoute status only if it differs from current status
    async fn update_http_route_status_if_needed(status_store: &Arc<dyn StatusStore>, resource: &HTTPRoute) {
        if let Some(expected_status) = resource_check::generate_http_route_status(resource) {
            let ns = resource.namespace().unwrap_or_else(|| "default".to_string());
            let name = resource.name_any();

            // Get current status and compare
            match status_store.get_http_route_status(&ns, &name).await {
                Ok(current_status) => {
                    if resource_check::http_route_status_needs_update(&current_status, &expected_status) {
                        // Status differs, need to update
                        if let Err(e) = status_store.update_http_route_status(&ns, &name, expected_status).await {
                            global_metrics().status_update_failed();
                            tracing::error!(
                                component = "k8s_controller",
                                kind = "HTTPRoute",
                                name = %name,
                                namespace = %ns,
                                error = %e,
                                "Failed to update HTTPRoute status"
                            );
                        } else {
                            global_metrics().status_update_success();
                            tracing::debug!(
                                component = "k8s_controller",
                                kind = "HTTPRoute",
                                name = %name,
                                namespace = %ns,
                                "HTTPRoute status updated"
                            );
                        }
                    } else {
                        // Status unchanged, skip update
                        global_metrics().status_update_skipped();
                        tracing::trace!(
                            component = "k8s_controller",
                            kind = "HTTPRoute",
                            name = %name,
                            namespace = %ns,
                            "HTTPRoute status unchanged, skipping update"
                        );
                    }
                }
                Err(e) => {
                    // Failed to get current status, try to update anyway
                    tracing::warn!(
                        component = "k8s_controller",
                        kind = "HTTPRoute",
                        name = %name,
                        namespace = %ns,
                        error = %e,
                        "Failed to get current HTTPRoute status, attempting update"
                    );
                    if let Err(e) = status_store.update_http_route_status(&ns, &name, expected_status).await {
                        global_metrics().status_update_failed();
                        tracing::error!(
                            component = "k8s_controller",
                            kind = "HTTPRoute",
                            name = %name,
                            namespace = %ns,
                            error = %e,
                            "Failed to update HTTPRoute status"
                        );
                    } else {
                        global_metrics().status_update_success();
                    }
                }
            }
        }
    }

    async fn watch_grpc_routes(&self) -> Result<()> {
        self.run_namespaced_watcher::<GRPCRoute, _>("GRPCRoute", |server, change, resource| {
            server.grpc_routes.apply_change(change, resource);
        })
        .await
    }

    async fn watch_tcp_routes(&self) -> Result<()> {
        self.run_namespaced_watcher::<TCPRoute, _>("TCPRoute", |server, change, resource| {
            server.tcp_routes.apply_change(change, resource);
        })
        .await
    }

    async fn watch_udp_routes(&self) -> Result<()> {
        self.run_namespaced_watcher::<UDPRoute, _>("UDPRoute", |server, change, resource| {
            server.udp_routes.apply_change(change, resource);
        })
        .await
    }

    async fn watch_tls_routes(&self) -> Result<()> {
        self.run_namespaced_watcher::<TLSRoute, _>("TLSRoute", |server, change, resource| {
            server.tls_routes.apply_change(change, resource);
        })
        .await
    }

    async fn watch_services(&self) -> Result<()> {
        self.run_namespaced_watcher::<Service, _>("Service", |server, change, resource| {
            server.services.apply_change(change, resource);
        })
        .await
    }

    async fn watch_endpoints(&self) -> Result<()> {
        self.run_namespaced_watcher::<Endpoints, _>("Endpoints", |server, change, resource| {
            server.endpoints.apply_change(change, resource);
        })
        .await
    }

    async fn watch_endpoint_slices(&self) -> Result<()> {
        self.run_namespaced_watcher::<EndpointSlice, _>("EndpointSlice", |server, change, resource| {
            server.endpoint_slices.apply_change(change, resource);
        })
        .await
    }

    async fn watch_secrets(&self) -> Result<()> {
        self.run_namespaced_watcher::<Secret, _>("Secret", |server, change, resource| {
            server.apply_secret_change(change, resource);
        })
        .await
    }

    async fn watch_reference_grants(&self) -> Result<()> {
        self.run_namespaced_watcher::<ReferenceGrant, _>("ReferenceGrant", |server, change, resource| {
            server.reference_grants.apply_change(change, resource);
        })
        .await
    }

    async fn watch_edgion_plugins(&self) -> Result<()> {
        self.run_namespaced_watcher::<EdgionPlugins, _>("EdgionPlugins", |server, change, resource| {
            server.edgion_plugins.apply_change(change, resource);
        })
        .await
    }

    async fn watch_edgion_stream_plugins(&self) -> Result<()> {
        self.run_namespaced_watcher::<EdgionStreamPlugins, _>("EdgionStreamPlugins", |server, change, resource| {
            server.edgion_stream_plugins.apply_change(change, resource);
        })
        .await
    }

    /// Watch EdgionTls with check before apply
    async fn watch_edgion_tls_with_check(&self) -> Result<()> {
        let kind = "EdgionTls";

        match &self.watch_mode {
            NamespaceWatchMode::AllNamespaces => {
                let api: Api<EdgionTls> = Api::all(self.client.clone());
                self.run_edgion_tls_watcher_with_check(api, kind).await
            }
            NamespaceWatchMode::SingleNamespace(ns) => {
                let api: Api<EdgionTls> = Api::namespaced(self.client.clone(), ns);
                self.run_edgion_tls_watcher_with_check(api, kind).await
            }
            NamespaceWatchMode::MultipleNamespaces(namespaces) => {
                let futures: Vec<_> = namespaces
                    .iter()
                    .map(|ns| {
                        let api: Api<EdgionTls> = Api::namespaced(self.client.clone(), ns);
                        let kind = kind.to_string();
                        let config = self.watcher_config();
                        let store = self.store.clone();
                        let config_server = self.config_server.clone();

                        async move {
                            let watcher = watcher(api, config);
                            tokio::pin!(watcher);
                            while let Some(event) = watcher
                                .try_next()
                                .await
                                .map_err(|e| anyhow::anyhow!("Watcher for {} failed: {}", kind, e))?
                            {
                                Self::handle_edgion_tls_event(&store, &config_server, event, &kind).await?;
                            }
                            Ok::<(), anyhow::Error>(())
                        }
                    })
                    .collect();

                futures::future::try_join_all(futures).await?;
                Ok(())
            }
        }
    }

    /// Run EdgionTls watcher with check
    async fn run_edgion_tls_watcher_with_check(&self, api: Api<EdgionTls>, kind: &str) -> Result<()> {
        let config = self.watcher_config();
        let watcher = watcher(api, config);
        tokio::pin!(watcher);
        while let Some(event) = watcher
            .try_next()
            .await
            .map_err(|e| anyhow::anyhow!("Watcher for {} failed: {}", kind, e))?
        {
            Self::handle_edgion_tls_event(&self.store, &self.config_server, event, kind).await?;
        }
        Ok(())
    }

    /// Handle EdgionTls event with check before apply
    async fn handle_edgion_tls_event(
        store: &KubernetesStore,
        config_server: &ConfigServer,
        event: watcher::Event<EdgionTls>,
        kind: &str,
    ) -> Result<()> {
        match event {
            watcher::Event::Apply(resource) => {
                Self::process_edgion_tls_apply(store, config_server, resource, kind, ResourceChange::EventAdd).await?;
            }
            watcher::Event::InitApply(resource) => {
                Self::process_edgion_tls_apply(store, config_server, resource, kind, ResourceChange::InitAdd).await?;
            }
            watcher::Event::Delete(resource) => {
                store
                    .remove_resource(kind, resource.namespace().as_deref(), &resource.name_any())
                    .await;
                config_server.apply_edgion_tls_change(ResourceChange::EventDelete, resource);
            }
            watcher::Event::Init => {}
            watcher::Event::InitDone => {
                config_server.set_cache_ready_by_kind(kind);
                tracing::info!("{} watcher initial sync complete, cache marked as ready", kind);
            }
        }
        Ok(())
    }

    /// Process EdgionTls apply/init-apply with validation
    async fn process_edgion_tls_apply(
        store: &KubernetesStore,
        config_server: &ConfigServer,
        resource: EdgionTls,
        kind: &str,
        change: ResourceChange,
    ) -> Result<()> {
        let yaml = serde_yaml::to_string(&resource)?;
        let name = resource.name_any();
        let ns = resource.namespace();

        // Always update KubernetesStore cache
        store
            .apply_resource(kind.to_string(), ns.clone(), name.clone(), yaml)
            .await;

        // Use resource_check to validate EdgionTls before apply
        let ctx = ResourceCheckContext::new(config_server);
        let check_result = check_edgion_tls(&ctx, &resource);

        if let Some(reason) = check_result.skip_reason {
            tracing::info!(
                component = "k8s_controller",
                kind = "EdgionTls",
                name = %name,
                namespace = ?ns,
                reason = %reason,
                "Skipping EdgionTls apply (still cached in KubernetesStore)"
            );
        } else {
            // Log warnings if any
            for warning in &check_result.warnings {
                tracing::warn!(
                    component = "k8s_controller",
                    kind = "EdgionTls",
                    name = %name,
                    namespace = ?ns,
                    warning = %warning,
                    "EdgionTls validation warning"
                );
            }
            config_server.apply_edgion_tls_change(change, resource);
        }
        Ok(())
    }

    async fn watch_backend_tls_policies(&self) -> Result<()> {
        self.run_namespaced_watcher::<BackendTLSPolicy, _>("BackendTLSPolicy", |server, change, resource| {
            server.backend_tls_policies.apply_change(change, resource);
        })
        .await
    }

    async fn watch_plugin_metadata(&self) -> Result<()> {
        self.run_namespaced_watcher::<PluginMetaData, _>("PluginMetaData", |server, change, resource| {
            server.plugin_metadata.apply_change(change, resource);
        })
        .await
    }

    async fn watch_link_sys(&self) -> Result<()> {
        self.run_namespaced_watcher::<LinkSys, _>("LinkSys", |server, change, resource| {
            server.link_sys.apply_change(change, resource);
        })
        .await
    }

    /// Watch Gateways with event-driven status updates
    async fn watch_gateways_with_status(&self) -> Result<()> {
        let kind = "Gateway";
        let status_store = self.status_store.clone();
        let gateway_class_name = self.gateway_class_name.clone();

        match &self.watch_mode {
            NamespaceWatchMode::AllNamespaces => {
                let api: Api<Gateway> = Api::all(self.client.clone());
                self.run_gateway_watcher_with_status(api, kind, status_store, gateway_class_name)
                    .await
            }
            NamespaceWatchMode::SingleNamespace(ns) => {
                let api: Api<Gateway> = Api::namespaced(self.client.clone(), ns);
                self.run_gateway_watcher_with_status(api, kind, status_store, gateway_class_name)
                    .await
            }
            NamespaceWatchMode::MultipleNamespaces(namespaces) => {
                let futures: Vec<_> = namespaces
                    .iter()
                    .map(|ns| {
                        let api: Api<Gateway> = Api::namespaced(self.client.clone(), ns);
                        let kind = kind.to_string();
                        let config = self.watcher_config();
                        let store = self.store.clone();
                        let config_server = self.config_server.clone();
                        let status_store = self.status_store.clone();
                        let gateway_class_name = self.gateway_class_name.clone();

                        async move {
                            let watcher = watcher(api, config);
                            tokio::pin!(watcher);
                            while let Some(event) = watcher
                                .try_next()
                                .await
                                .map_err(|e| anyhow::anyhow!("Watcher for {} failed: {}", kind, e))?
                            {
                                Self::handle_gateway_event(
                                    &store,
                                    &config_server,
                                    &status_store,
                                    event,
                                    &kind,
                                    &gateway_class_name,
                                )
                                .await?;
                            }
                            Ok::<(), anyhow::Error>(())
                        }
                    })
                    .collect();

                futures::future::try_join_all(futures).await?;
                Ok(())
            }
        }
    }

    /// Run Gateway watcher with status updates
    async fn run_gateway_watcher_with_status(
        &self,
        api: Api<Gateway>,
        kind: &str,
        status_store: Arc<dyn StatusStore>,
        gateway_class_name: String,
    ) -> Result<()> {
        let config = self.watcher_config();
        let watcher = watcher(api, config);
        tokio::pin!(watcher);
        while let Some(event) = watcher
            .try_next()
            .await
            .map_err(|e| anyhow::anyhow!("Watcher for {} failed: {}", kind, e))?
        {
            Self::handle_gateway_event(
                &self.store,
                &self.config_server,
                &status_store,
                event,
                kind,
                &gateway_class_name,
            )
            .await?;
        }
        Ok(())
    }

    /// Handle Gateway event with status update
    async fn handle_gateway_event(
        store: &KubernetesStore,
        config_server: &ConfigServer,
        status_store: &Arc<dyn StatusStore>,
        event: watcher::Event<Gateway>,
        kind: &str,
        gateway_class_name: &str,
    ) -> Result<()> {
        match event {
            watcher::Event::Apply(resource) => {
                Self::process_gateway_apply(store, config_server, status_store, resource, kind, gateway_class_name, ResourceChange::EventAdd).await?;
            }
            watcher::Event::InitApply(resource) => {
                Self::process_gateway_apply(store, config_server, status_store, resource, kind, gateway_class_name, ResourceChange::InitAdd).await?;
            }
            watcher::Event::Delete(resource) => {
                store
                    .remove_resource(kind, resource.namespace().as_deref(), &resource.name_any())
                    .await;
                config_server.apply_gateway_change(ResourceChange::EventDelete, resource);
            }
            watcher::Event::Init => {}
            watcher::Event::InitDone => {
                config_server.set_cache_ready_by_kind(kind);
                tracing::info!("{} watcher initial sync complete, cache marked as ready", kind);
            }
        }
        Ok(())
    }

    /// Process Gateway apply/init-apply with status update
    async fn process_gateway_apply(
        store: &KubernetesStore,
        config_server: &ConfigServer,
        status_store: &Arc<dyn StatusStore>,
        resource: Gateway,
        kind: &str,
        gateway_class_name: &str,
        change: ResourceChange,
    ) -> Result<()> {
        let yaml = serde_yaml::to_string(&resource)?;

        store
            .apply_resource(kind.to_string(), resource.namespace(), resource.name_any(), yaml)
            .await;
        config_server.apply_gateway_change(change, resource.clone());

        // Event-driven status update with comparison
        Self::update_gateway_status_if_needed(status_store, &resource, gateway_class_name).await;
        Ok(())
    }

    /// Update Gateway status only if it differs from current status
    async fn update_gateway_status_if_needed(
        status_store: &Arc<dyn StatusStore>,
        resource: &Gateway,
        gateway_class_name: &str,
    ) {
        if let Some(expected_status) = resource_check::generate_gateway_status(resource, gateway_class_name) {
            let ns = resource.namespace().unwrap_or_else(|| "default".to_string());
            let name = resource.name_any();

            // Get current status and compare
            match status_store.get_gateway_status(&ns, &name).await {
                Ok(current_status) => {
                    if resource_check::gateway_status_needs_update(&current_status, &expected_status) {
                        // Status differs, need to update
                        if let Err(e) = status_store.update_gateway_status(&ns, &name, expected_status).await {
                            global_metrics().status_update_failed();
                            tracing::error!(
                                component = "k8s_controller",
                                kind = "Gateway",
                                name = %name,
                                namespace = %ns,
                                error = %e,
                                "Failed to update Gateway status"
                            );
                        } else {
                            global_metrics().status_update_success();
                            tracing::debug!(
                                component = "k8s_controller",
                                kind = "Gateway",
                                name = %name,
                                namespace = %ns,
                                "Gateway status updated"
                            );
                        }
                    } else {
                        // Status unchanged, skip update
                        global_metrics().status_update_skipped();
                        tracing::trace!(
                            component = "k8s_controller",
                            kind = "Gateway",
                            name = %name,
                            namespace = %ns,
                            "Gateway status unchanged, skipping update"
                        );
                    }
                }
                Err(e) => {
                    // Failed to get current status, try to update anyway
                    tracing::warn!(
                        component = "k8s_controller",
                        kind = "Gateway",
                        name = %name,
                        namespace = %ns,
                        error = %e,
                        "Failed to get current Gateway status, attempting update"
                    );
                    if let Err(e) = status_store.update_gateway_status(&ns, &name, expected_status).await {
                        global_metrics().status_update_failed();
                        tracing::error!(
                            component = "k8s_controller",
                            kind = "Gateway",
                            name = %name,
                            namespace = %ns,
                            error = %e,
                            "Failed to update Gateway status"
                        );
                    } else {
                        global_metrics().status_update_success();
                    }
                }
            }
        }
    }

    // =========================================================================
    // Cluster-scoped resource watchers
    // These always watch all namespaces (cluster-wide)
    // =========================================================================

    async fn watch_gateway_classes(&self) -> Result<()> {
        // GatewayClass is cluster-scoped, always use Api::all
        let api: Api<GatewayClass> = Api::all(self.client.clone());
        let config = self.watcher_config();
        let watcher = watcher(api, config);
        tokio::pin!(watcher);
        while let Some(event) = watcher
            .try_next()
            .await
            .map_err(|e| anyhow::anyhow!("Watcher for GatewayClass failed: {}", e))?
        {
            self.handle_event(event, "GatewayClass", |server, change, resource| {
                server.gateway_classes.apply_change(change, resource);
            })
            .await?;
        }
        Ok(())
    }

    async fn watch_edgion_gateway_configs(&self) -> Result<()> {
        // EdgionGatewayConfig is cluster-scoped (namespaced = false), always use Api::all
        let api: Api<EdgionGatewayConfig> = Api::all(self.client.clone());
        let config = self.watcher_config();
        let watcher = watcher(api, config);
        tokio::pin!(watcher);
        while let Some(event) = watcher
            .try_next()
            .await
            .map_err(|e| anyhow::anyhow!("Watcher for EdgionGatewayConfig failed: {}", e))?
        {
            self.handle_event(event, "EdgionGatewayConfig", |server, change, resource| {
                server.edgion_gateway_configs.apply_change(change, resource);
            })
            .await?;
        }
        Ok(())
    }
}
