//! Kubernetes Controller using kube-runtime
//!
//! Uses kube-runtime's Controller pattern for event-driven reconciliation:
//! - Each resource type has its own Controller with reflector::Store
//! - Initial sync: wait for Store ready, then apply all with InitAdd
//! - Runtime: event-driven reconcile with EventUpdate/EventDelete

use anyhow::Result;
use futures::StreamExt;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::reflector::Store;
use kube::runtime::{reflector, watcher, Controller};
use kube::{Api, Client, Resource};
use serde::de::DeserializeOwned;
use std::sync::Arc;

use super::context::ControllerContext;
use super::error::error_policy;
use super::namespace::NamespaceWatchMode;
use super::reconcilers::*;
use super::status::{KubernetesStatusStore, StatusStore};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::*;

/// Kubernetes Controller using kube-runtime
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
            "Creating Kubernetes controller with kube-runtime Controller pattern"
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

    /// Create API for namespaced resources based on watch mode
    fn create_namespaced_api<K>(&self) -> Api<K>
    where
        K: Resource<Scope = kube::core::NamespaceResourceScope>,
        <K as Resource>::DynamicType: Default,
    {
        match &self.watch_mode {
            NamespaceWatchMode::AllNamespaces => Api::all(self.client.clone()),
            NamespaceWatchMode::SingleNamespace(ns) => Api::namespaced(self.client.clone(), ns),
            // For multiple namespaces, we use Api::all() and filter in reconcile
            // A proper implementation would run separate controllers per namespace
            NamespaceWatchMode::MultipleNamespaces(_) => Api::all(self.client.clone()),
        }
    }

    /// Check if a resource namespace matches our watch mode
    /// Reserved for future use when implementing namespace filtering in reconcilers
    #[allow(dead_code)]
    fn should_process_namespace(&self, namespace: Option<&str>) -> bool {
        match &self.watch_mode {
            NamespaceWatchMode::AllNamespaces => true,
            NamespaceWatchMode::SingleNamespace(ns) => namespace == Some(ns.as_str()),
            NamespaceWatchMode::MultipleNamespaces(namespaces) => {
                namespace.map_or(false, |ns| namespaces.iter().any(|n| n == ns))
            }
        }
    }

    /// Create API for cluster-scoped resources
    fn create_cluster_api<K>(&self) -> Api<K>
    where
        K: Resource<Scope = kube::core::ClusterResourceScope>,
        <K as Resource>::DynamicType: Default,
    {
        Api::all(self.client.clone())
    }

    /// Run the controller
    pub async fn run(&self) -> Result<()> {
        tracing::info!(
            component = "k8s_controller",
            "Starting Kubernetes controller with reflector-based initial sync"
        );

        let watcher_config = self.watcher_config();

        // Create APIs
        let http_route_api: Api<HTTPRoute> = self.create_namespaced_api();
        let grpc_route_api: Api<GRPCRoute> = self.create_namespaced_api();
        let tcp_route_api: Api<TCPRoute> = self.create_namespaced_api();
        let udp_route_api: Api<UDPRoute> = self.create_namespaced_api();
        let tls_route_api: Api<TLSRoute> = self.create_namespaced_api();
        let gateway_api: Api<Gateway> = self.create_namespaced_api();
        let gateway_class_api: Api<GatewayClass> = self.create_cluster_api();
        let service_api: Api<Service> = self.create_namespaced_api();
        let endpoints_api: Api<Endpoints> = self.create_namespaced_api();
        let endpoint_slices_api: Api<EndpointSlice> = self.create_namespaced_api();
        let secret_api: Api<Secret> = self.create_namespaced_api();
        let reference_grant_api: Api<ReferenceGrant> = self.create_namespaced_api();
        let edgion_tls_api: Api<EdgionTls> = self.create_namespaced_api();
        let edgion_plugins_api: Api<EdgionPlugins> = self.create_namespaced_api();
        let edgion_stream_plugins_api: Api<EdgionStreamPlugins> = self.create_namespaced_api();
        let backend_tls_policies_api: Api<BackendTLSPolicy> = self.create_namespaced_api();
        let plugin_metadata_api: Api<PluginMetaData> = self.create_namespaced_api();
        let link_sys_api: Api<LinkSys> = self.create_namespaced_api();
        let edgion_gateway_configs_api: Api<EdgionGatewayConfig> = self.create_cluster_api();

        // ==================== Phase 1: Create reflector stores ====================
        tracing::info!(component = "k8s_controller", "Creating reflector stores for initial sync");

        // Create store + writer pairs for each resource type
        let (http_route_store, http_route_writer) = reflector::store();
        let (grpc_route_store, grpc_route_writer) = reflector::store();
        let (tcp_route_store, tcp_route_writer) = reflector::store();
        let (udp_route_store, udp_route_writer) = reflector::store();
        let (tls_route_store, tls_route_writer) = reflector::store();
        let (gateway_store, gateway_writer) = reflector::store();
        let (gateway_class_store, gateway_class_writer) = reflector::store();
        let (service_store, service_writer) = reflector::store();
        let (endpoints_store, endpoints_writer) = reflector::store();
        let (endpoint_slice_store, endpoint_slice_writer) = reflector::store();
        let (secret_store, secret_writer) = reflector::store();
        let (reference_grant_store, reference_grant_writer) = reflector::store();
        let (edgion_tls_store, edgion_tls_writer) = reflector::store();
        let (edgion_plugins_store, edgion_plugins_writer) = reflector::store();
        let (edgion_stream_plugins_store, edgion_stream_plugins_writer) = reflector::store();
        let (backend_tls_policy_store, backend_tls_policy_writer) = reflector::store();
        let (plugin_metadata_store, plugin_metadata_writer) = reflector::store();
        let (link_sys_store, link_sys_writer) = reflector::store();
        let (edgion_gateway_config_store, edgion_gateway_config_writer) = reflector::store();

        // Start reflectors in background (they will LIST then WATCH)
        let wc = watcher_config.clone();
        tokio::spawn(reflector(http_route_writer, watcher(http_route_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(grpc_route_writer, watcher(grpc_route_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(tcp_route_writer, watcher(tcp_route_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(udp_route_writer, watcher(udp_route_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(tls_route_writer, watcher(tls_route_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(gateway_writer, watcher(gateway_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(gateway_class_writer, watcher(gateway_class_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(service_writer, watcher(service_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(endpoints_writer, watcher(endpoints_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(endpoint_slice_writer, watcher(endpoint_slices_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(secret_writer, watcher(secret_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(reference_grant_writer, watcher(reference_grant_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(edgion_tls_writer, watcher(edgion_tls_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(edgion_plugins_writer, watcher(edgion_plugins_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(edgion_stream_plugins_writer, watcher(edgion_stream_plugins_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(backend_tls_policy_writer, watcher(backend_tls_policies_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(plugin_metadata_writer, watcher(plugin_metadata_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(link_sys_writer, watcher(link_sys_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));
        tokio::spawn(reflector(edgion_gateway_config_writer, watcher(edgion_gateway_configs_api.clone(), wc.clone())).for_each(|_| futures::future::ready(())));

        // ==================== Phase 2: Wait for all stores to be ready ====================
        tracing::info!(component = "k8s_controller", "Waiting for all reflector stores to be ready (initial LIST complete)");

        tokio::try_join!(
            async { http_route_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("HTTPRoute store error: {}", e)) },
            async { grpc_route_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("GRPCRoute store error: {}", e)) },
            async { tcp_route_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("TCPRoute store error: {}", e)) },
            async { udp_route_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("UDPRoute store error: {}", e)) },
            async { tls_route_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("TLSRoute store error: {}", e)) },
            async { gateway_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("Gateway store error: {}", e)) },
            async { gateway_class_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("GatewayClass store error: {}", e)) },
            async { service_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("Service store error: {}", e)) },
            async { endpoints_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("Endpoints store error: {}", e)) },
            async { endpoint_slice_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("EndpointSlice store error: {}", e)) },
            async { secret_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("Secret store error: {}", e)) },
            async { reference_grant_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("ReferenceGrant store error: {}", e)) },
            async { edgion_tls_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("EdgionTls store error: {}", e)) },
            async { edgion_plugins_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("EdgionPlugins store error: {}", e)) },
            async { edgion_stream_plugins_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("EdgionStreamPlugins store error: {}", e)) },
            async { backend_tls_policy_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("BackendTLSPolicy store error: {}", e)) },
            async { plugin_metadata_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("PluginMetaData store error: {}", e)) },
            async { link_sys_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("LinkSys store error: {}", e)) },
            async { edgion_gateway_config_store.wait_until_ready().await.map_err(|e| anyhow::anyhow!("EdgionGatewayConfig store error: {}", e)) },
        )?;

        tracing::info!(component = "k8s_controller", "All reflector stores ready, applying initial state");

        // ==================== Phase 3: Apply initial state with InitAdd ====================
        self.apply_initial_state(
            &http_route_store,
            &grpc_route_store,
            &tcp_route_store,
            &udp_route_store,
            &tls_route_store,
            &gateway_store,
            &gateway_class_store,
            &service_store,
            &endpoints_store,
            &endpoint_slice_store,
            &secret_store,
            &reference_grant_store,
            &edgion_tls_store,
            &edgion_plugins_store,
            &edgion_stream_plugins_store,
            &backend_tls_policy_store,
            &plugin_metadata_store,
            &link_sys_store,
            &edgion_gateway_config_store,
        );

        tracing::info!(component = "k8s_controller", "Initial state applied, starting event-driven controllers");

        // ==================== Phase 4: Start Controllers (event-driven) ====================
        let ctx = Arc::new(ControllerContext {
            config_server: self.config_server.clone(),
            status_store: self.status_store.clone(),
            gateway_class_name: self.gateway_class_name.clone(),
        });

        tracing::info!(component = "k8s_controller", "Starting all Controllers");

        let mut handles = Vec::new();

        // Spawn all controllers
        handles.push(self.spawn_controller("HTTPRoute", http_route_api, watcher_config.clone(), ctx.clone(), reconcile_http_route));
        handles.push(self.spawn_controller("GRPCRoute", grpc_route_api, watcher_config.clone(), ctx.clone(), reconcile_grpc_route));
        handles.push(self.spawn_controller("TCPRoute", tcp_route_api, watcher_config.clone(), ctx.clone(), reconcile_tcp_route));
        handles.push(self.spawn_controller("UDPRoute", udp_route_api, watcher_config.clone(), ctx.clone(), reconcile_udp_route));
        handles.push(self.spawn_controller("TLSRoute", tls_route_api, watcher_config.clone(), ctx.clone(), reconcile_tls_route));
        handles.push(self.spawn_controller("Gateway", gateway_api, watcher_config.clone(), ctx.clone(), reconcile_gateway));
        handles.push(self.spawn_controller("GatewayClass", gateway_class_api, watcher_config.clone(), ctx.clone(), reconcile_gateway_class));
        handles.push(self.spawn_controller("Service", service_api, watcher_config.clone(), ctx.clone(), reconcile_service));
        handles.push(self.spawn_controller("Endpoints", endpoints_api, watcher_config.clone(), ctx.clone(), reconcile_endpoints));
        handles.push(self.spawn_controller("EndpointSlice", endpoint_slices_api, watcher_config.clone(), ctx.clone(), reconcile_endpoint_slice));
        handles.push(self.spawn_controller("Secret", secret_api, watcher_config.clone(), ctx.clone(), reconcile_secret));
        handles.push(self.spawn_controller("ReferenceGrant", reference_grant_api, watcher_config.clone(), ctx.clone(), reconcile_reference_grant));
        handles.push(self.spawn_controller("EdgionTls", edgion_tls_api, watcher_config.clone(), ctx.clone(), reconcile_edgion_tls));
        handles.push(self.spawn_controller("EdgionPlugins", edgion_plugins_api, watcher_config.clone(), ctx.clone(), reconcile_edgion_plugins));
        handles.push(self.spawn_controller("EdgionStreamPlugins", edgion_stream_plugins_api, watcher_config.clone(), ctx.clone(), reconcile_edgion_stream_plugins));
        handles.push(self.spawn_controller("BackendTLSPolicy", backend_tls_policies_api, watcher_config.clone(), ctx.clone(), reconcile_backend_tls_policy));
        handles.push(self.spawn_controller("PluginMetaData", plugin_metadata_api, watcher_config.clone(), ctx.clone(), reconcile_plugin_metadata));
        handles.push(self.spawn_controller("LinkSys", link_sys_api, watcher_config.clone(), ctx.clone(), reconcile_link_sys));
        handles.push(self.spawn_controller("EdgionGatewayConfig", edgion_gateway_configs_api, watcher_config.clone(), ctx.clone(), reconcile_edgion_gateway_config));

        tracing::info!(
            component = "k8s_controller",
            count = handles.len(),
            "All controllers spawned (initial sync already complete, now event-driven)"
        );

        futures::future::join_all(handles).await;

        tracing::warn!(component = "k8s_controller", "All controllers have stopped");
        Ok(())
    }

    /// Spawn a controller for a specific resource type
    fn spawn_controller<K, ReconcileFn, ReconcileFut>(
        &self,
        kind: &'static str,
        api: Api<K>,
        watcher_config: watcher::Config,
        ctx: Arc<ControllerContext>,
        reconcile: ReconcileFn,
    ) -> tokio::task::JoinHandle<()>
    where
        K: Resource + Clone + Send + Sync + std::fmt::Debug + DeserializeOwned + 'static,
        K::DynamicType: Default + Eq + std::hash::Hash + Clone + std::fmt::Debug + Unpin,
        ReconcileFn: FnMut(Arc<K>, Arc<ControllerContext>) -> ReconcileFut + Send + 'static,
        ReconcileFut: std::future::Future<Output = Result<kube::runtime::controller::Action, super::error::ReconcileError>> + Send + 'static,
    {
        tokio::spawn(async move {
            Controller::new(api, watcher_config)
                .run(reconcile, error_policy, ctx)
                .for_each(|res| async move {
                    match res {
                        Ok((obj, _action)) => tracing::trace!(obj = ?obj, kind = kind, "Reconciled"),
                        Err(e) => tracing::error!(error = %e, kind = kind, "Controller error"),
                    }
                })
                .await;
        })
    }

    /// Apply initial state from reflector stores to ConfigServer
    fn apply_initial_state(
        &self,
        http_route_store: &Store<HTTPRoute>,
        grpc_route_store: &Store<GRPCRoute>,
        tcp_route_store: &Store<TCPRoute>,
        udp_route_store: &Store<UDPRoute>,
        tls_route_store: &Store<TLSRoute>,
        gateway_store: &Store<Gateway>,
        gateway_class_store: &Store<GatewayClass>,
        service_store: &Store<Service>,
        endpoints_store: &Store<Endpoints>,
        endpoint_slice_store: &Store<EndpointSlice>,
        secret_store: &Store<Secret>,
        reference_grant_store: &Store<ReferenceGrant>,
        edgion_tls_store: &Store<EdgionTls>,
        edgion_plugins_store: &Store<EdgionPlugins>,
        edgion_stream_plugins_store: &Store<EdgionStreamPlugins>,
        backend_tls_policy_store: &Store<BackendTLSPolicy>,
        plugin_metadata_store: &Store<PluginMetaData>,
        link_sys_store: &Store<LinkSys>,
        edgion_gateway_config_store: &Store<EdgionGatewayConfig>,
    ) {
        // HTTPRoute
        for route in http_route_store.state() {
            self.config_server.routes.apply_change(ResourceChange::InitAdd, (*route).clone());
        }
        self.config_server.set_cache_ready_by_kind("HTTPRoute");
        tracing::debug!(component = "k8s_controller", kind = "HTTPRoute", count = http_route_store.state().len(), "Initial state applied");

        // GRPCRoute
        for route in grpc_route_store.state() {
            self.config_server.grpc_routes.apply_change(ResourceChange::InitAdd, (*route).clone());
        }
        self.config_server.set_cache_ready_by_kind("GRPCRoute");
        tracing::debug!(component = "k8s_controller", kind = "GRPCRoute", count = grpc_route_store.state().len(), "Initial state applied");

        // TCPRoute
        for route in tcp_route_store.state() {
            self.config_server.tcp_routes.apply_change(ResourceChange::InitAdd, (*route).clone());
        }
        self.config_server.set_cache_ready_by_kind("TCPRoute");
        tracing::debug!(component = "k8s_controller", kind = "TCPRoute", count = tcp_route_store.state().len(), "Initial state applied");

        // UDPRoute
        for route in udp_route_store.state() {
            self.config_server.udp_routes.apply_change(ResourceChange::InitAdd, (*route).clone());
        }
        self.config_server.set_cache_ready_by_kind("UDPRoute");
        tracing::debug!(component = "k8s_controller", kind = "UDPRoute", count = udp_route_store.state().len(), "Initial state applied");

        // TLSRoute
        for route in tls_route_store.state() {
            self.config_server.tls_routes.apply_change(ResourceChange::InitAdd, (*route).clone());
        }
        self.config_server.set_cache_ready_by_kind("TLSRoute");
        tracing::debug!(component = "k8s_controller", kind = "TLSRoute", count = tls_route_store.state().len(), "Initial state applied");

        // Gateway (filter by gateway class)
        for gateway in gateway_store.state() {
            if gateway.spec.gateway_class_name == self.gateway_class_name {
                self.config_server.apply_gateway_change(ResourceChange::InitAdd, (*gateway).clone());
            }
        }
        self.config_server.set_cache_ready_by_kind("Gateway");
        tracing::debug!(component = "k8s_controller", kind = "Gateway", count = gateway_store.state().len(), "Initial state applied");

        // GatewayClass
        for class in gateway_class_store.state() {
            self.config_server.gateway_classes.apply_change(ResourceChange::InitAdd, (*class).clone());
        }
        self.config_server.set_cache_ready_by_kind("GatewayClass");
        tracing::debug!(component = "k8s_controller", kind = "GatewayClass", count = gateway_class_store.state().len(), "Initial state applied");

        // Service
        for service in service_store.state() {
            self.config_server.services.apply_change(ResourceChange::InitAdd, (*service).clone());
        }
        self.config_server.set_cache_ready_by_kind("Service");
        tracing::debug!(component = "k8s_controller", kind = "Service", count = service_store.state().len(), "Initial state applied");

        // Endpoints
        for endpoints in endpoints_store.state() {
            self.config_server.endpoints.apply_change(ResourceChange::InitAdd, (*endpoints).clone());
        }
        self.config_server.set_cache_ready_by_kind("Endpoints");
        tracing::debug!(component = "k8s_controller", kind = "Endpoints", count = endpoints_store.state().len(), "Initial state applied");

        // EndpointSlice
        for slice in endpoint_slice_store.state() {
            self.config_server.endpoint_slices.apply_change(ResourceChange::InitAdd, (*slice).clone());
        }
        self.config_server.set_cache_ready_by_kind("EndpointSlice");
        tracing::debug!(component = "k8s_controller", kind = "EndpointSlice", count = endpoint_slice_store.state().len(), "Initial state applied");

        // Secret
        for secret in secret_store.state() {
            self.config_server.apply_secret_change(ResourceChange::InitAdd, (*secret).clone());
        }
        self.config_server.set_cache_ready_by_kind("Secret");
        tracing::debug!(component = "k8s_controller", kind = "Secret", count = secret_store.state().len(), "Initial state applied");

        // ReferenceGrant
        for grant in reference_grant_store.state() {
            self.config_server.reference_grants.apply_change(ResourceChange::InitAdd, (*grant).clone());
        }
        self.config_server.set_cache_ready_by_kind("ReferenceGrant");
        tracing::debug!(component = "k8s_controller", kind = "ReferenceGrant", count = reference_grant_store.state().len(), "Initial state applied");

        // EdgionTls
        for tls in edgion_tls_store.state() {
            self.config_server.apply_edgion_tls_change(ResourceChange::InitAdd, (*tls).clone());
        }
        self.config_server.set_cache_ready_by_kind("EdgionTls");
        tracing::debug!(component = "k8s_controller", kind = "EdgionTls", count = edgion_tls_store.state().len(), "Initial state applied");

        // EdgionPlugins
        for plugins in edgion_plugins_store.state() {
            self.config_server.edgion_plugins.apply_change(ResourceChange::InitAdd, (*plugins).clone());
        }
        self.config_server.set_cache_ready_by_kind("EdgionPlugins");
        tracing::debug!(component = "k8s_controller", kind = "EdgionPlugins", count = edgion_plugins_store.state().len(), "Initial state applied");

        // EdgionStreamPlugins
        for plugins in edgion_stream_plugins_store.state() {
            self.config_server.edgion_stream_plugins.apply_change(ResourceChange::InitAdd, (*plugins).clone());
        }
        self.config_server.set_cache_ready_by_kind("EdgionStreamPlugins");
        tracing::debug!(component = "k8s_controller", kind = "EdgionStreamPlugins", count = edgion_stream_plugins_store.state().len(), "Initial state applied");

        // BackendTLSPolicy
        for policy in backend_tls_policy_store.state() {
            self.config_server.backend_tls_policies.apply_change(ResourceChange::InitAdd, (*policy).clone());
        }
        self.config_server.set_cache_ready_by_kind("BackendTLSPolicy");
        tracing::debug!(component = "k8s_controller", kind = "BackendTLSPolicy", count = backend_tls_policy_store.state().len(), "Initial state applied");

        // PluginMetaData
        for metadata in plugin_metadata_store.state() {
            self.config_server.plugin_metadata.apply_change(ResourceChange::InitAdd, (*metadata).clone());
        }
        self.config_server.set_cache_ready_by_kind("PluginMetaData");
        tracing::debug!(component = "k8s_controller", kind = "PluginMetaData", count = plugin_metadata_store.state().len(), "Initial state applied");

        // LinkSys
        for link in link_sys_store.state() {
            self.config_server.link_sys.apply_change(ResourceChange::InitAdd, (*link).clone());
        }
        self.config_server.set_cache_ready_by_kind("LinkSys");
        tracing::debug!(component = "k8s_controller", kind = "LinkSys", count = link_sys_store.state().len(), "Initial state applied");

        // EdgionGatewayConfig
        for config in edgion_gateway_config_store.state() {
            self.config_server.edgion_gateway_configs.apply_change(ResourceChange::InitAdd, (*config).clone());
        }
        self.config_server.set_cache_ready_by_kind("EdgionGatewayConfig");
        tracing::debug!(component = "k8s_controller", kind = "EdgionGatewayConfig", count = edgion_gateway_config_store.state().len(), "Initial state applied");

        tracing::info!(component = "k8s_controller", "All initial state applied to ConfigServer");
    }
}
