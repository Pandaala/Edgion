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

use anyhow::Result;
use futures::TryStreamExt;
use kube::core::NamespaceResourceScope;
use kube::runtime::watcher;
use kube::{Api, Client};
use std::sync::Arc;

use super::{KubernetesStore, StatusReconciler};
use crate::core::conf_mgr::{KubernetesStatusStore, StatusStore};
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{CacheEventDispatch, ConfigServer};
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
    reconciler: StatusReconciler,
    #[allow(dead_code)]
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
    pub async fn run(&self) -> Result<()> {
        tracing::info!(
            component = "k8s_controller",
            event = "controller_start",
            watch_mode = ?self.watch_mode,
            "Starting Kubernetes controller"
        );

        // Run all watchers concurrently
        // Note: If any watcher fails, all will be cancelled
        //
        // Resource types:
        // - Cluster-scoped: GatewayClass (always watch all)
        // - Namespace-scoped: Everything else (respects watch_mode)
        tokio::try_join!(
            // Status reconciler
            async {
                self.reconciler.run().await;
                Ok(())
            },
            // Cluster-scoped resources (always watch all namespaces)
            self.watch_gateway_classes(),
            // Namespace-scoped base conf resources
            self.watch_gateways(),
            self.watch_edgion_gateway_configs(),
            // Route resources
            self.watch_http_routes(),
            self.watch_grpc_routes(),
            self.watch_tcp_routes(),
            self.watch_udp_routes(),
            self.watch_tls_routes(),
            // Backend resources
            self.watch_services(),
            self.watch_endpoints(),
            self.watch_endpoint_slices(),
            // Other resources
            self.watch_secrets(),
            self.watch_reference_grants(),
            self.watch_edgion_plugins(),
            self.watch_edgion_stream_plugins(),
            self.watch_edgion_tls(),
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

    async fn watch_http_routes(&self) -> Result<()> {
        self.run_namespaced_watcher::<HTTPRoute, _>("HTTPRoute", |server, change, resource| {
            server.routes.apply_change(change, resource);
        })
        .await
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

    async fn watch_edgion_tls(&self) -> Result<()> {
        self.run_namespaced_watcher::<EdgionTls, _>("EdgionTls", |server, change, resource| {
            server.apply_edgion_tls_change(change, resource);
        })
        .await
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

    async fn watch_gateways(&self) -> Result<()> {
        self.run_namespaced_watcher::<Gateway, _>("Gateway", |server, change, resource| {
            server.gateways.apply_change(change, resource);
        })
        .await
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
