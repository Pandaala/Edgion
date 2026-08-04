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
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::watcher;
use kube::runtime::WatchStreamExt;
use kube::{Client, Resource, ResourceExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::namespace::NamespaceWatchMode;
use super::resource_controller::{ApiScope, RelinkReason, RelinkSignalSender, ResourceController};
use super::ShutdownSignal;
use crate::conf_mgr::conf_center::MetadataFilterConfig;
use crate::conf_mgr::sync_runtime::resource_processor::{ProcessorHandler, ResourceProcessor};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::*;
use edgion_resources::ResourceMeta;

// Import handlers from conf_mgr
use crate::conf_mgr::sync_runtime::resource_processor::{
    BackendTlsPolicyAlpha3Handler, BackendTlsPolicyHandler, ConfigMapHandler, EdgionAIServiceHandler,
    EdgionAcmeHandler, EdgionBackendTrafficPolicyHandler, EdgionConfigDataHandler, EdgionGatewayConfigHandler,
    EdgionPluginsHandler, EdgionStreamPluginsHandler, EdgionTlsHandler, EndpointSliceHandler, GatewayClassHandler,
    GatewayHandler, GrpcRouteHandler, HttpRouteHandler, LinkSysHandler, ReferenceGrantBetaHandler,
    ReferenceGrantHandler, SecretHandler, ServiceHandler, TcpRouteHandler, TlsRouteAlpha3Handler, TlsRouteHandler,
    UdpRouteHandler,
};

/// Context for spawn functions
struct SpawnContext {
    watcher_config: watcher::Config,
    shutdown: ShutdownSignal,
    relink_tx: RelinkSignalSender,
    metadata_filter: Option<MetadataFilterConfig>,
    leader_handle: Option<super::leader_election::LeaderHandle>,
    runtime: Arc<ControllerRuntime>,
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
    let capacity = crate::config::get_cache_capacity(kind);
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        capacity,
        Arc::new(handler),
        ctx.runtime.clone(),
    ));

    // 2. Set metadata filter if configured
    if let Some(ref filter) = ctx.metadata_filter {
        processor.set_metadata_filter(filter.clone());
    }

    // 3. Register to processor_registry via runtime
    ctx.runtime.processor_registry.register(processor.clone());

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
    let capacity = crate::config::get_cache_capacity(kind);
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        capacity,
        Arc::new(handler),
        ctx.runtime.clone(),
    ));

    // 2. Set metadata filter if configured
    if let Some(ref filter) = ctx.metadata_filter {
        processor.set_metadata_filter(filter.clone());
    }

    // 3. Register to processor_registry via runtime
    ctx.runtime.processor_registry.register(processor.clone());

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
/// pipeline. It only populates the NamespaceStore in ControllerRuntime for Selector
/// namespace policy evaluation.
fn spawn_namespace_watcher(
    client: Client,
    watcher_config: watcher::Config,
    shutdown: ShutdownSignal,
    runtime: Arc<ControllerRuntime>,
) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        use kube::runtime::watcher::Event;

        let api: kube::Api<Namespace> = kube::Api::all(client);
        let store = &runtime.namespace_store;

        tracing::info!(component = "namespace_watcher", "Starting Namespace label watcher");

        let stream = watcher::watcher(api, watcher_config).default_backoff();

        futures::pin_mut!(stream);

        let mut init_batch: Option<Vec<Namespace>> = None;
        let mut shutdown = shutdown;
        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    tracing::info!(component = "namespace_watcher", "Shutdown signal received");
                    break;
                }
                event = stream.try_next() => {
                    match event {
                        Ok(Some(ev)) => match ev {
                            Event::Init => {
                                init_batch = Some(Vec::new());
                            }
                            Event::InitApply(ns) => {
                                if let Some(ref mut batch) = init_batch {
                                    batch.push(ns);
                                }
                            }
                            Event::InitDone => {
                                if let Some(batch) = init_batch.take() {
                                    let count = batch.len();
                                    store.replace_all(batch);
                                    tracing::info!(
                                        component = "namespace_watcher",
                                        count = count,
                                        "NamespaceStore authoritative replace_all on init_done"
                                    );
                                }
                            }
                            Event::Apply(ns) => {
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
                                    requeue_selector_gateways(runtime.clone()).await;
                                }
                            }
                            Event::Delete(ns) => {
                                let name = ns.name_any();
                                if store.remove(&name) {
                                    tracing::debug!(
                                        component = "namespace_watcher",
                                        namespace = %name,
                                        "Namespace deleted, requeuing Selector Gateways"
                                    );
                                    requeue_selector_gateways(runtime.clone()).await;
                                }
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
async fn requeue_selector_gateways(runtime: Arc<ControllerRuntime>) {
    let Some(processor) = runtime.processor_registry.get("Gateway") else {
        return;
    };
    // Iterate cache keys (already filtered by our gatewayClassName via the
    // handler) rather than raw_store, to avoid requeuing Gateways owned by
    // other controllers.
    for key in processor.list_keys() {
        let Some(any) = processor.get_raw_any(&key) else {
            continue;
        };
        let Ok(gw) = any.downcast::<edgion_resources::prelude_resources::Gateway>() else {
            continue;
        };
        let uses_selector = gw.spec.listeners.as_deref().unwrap_or_default().iter().any(|l| {
            l.allowed_routes
                .as_ref()
                .and_then(|ar| ar.namespaces.as_ref())
                .and_then(|ns| ns.from.as_deref())
                == Some("Selector")
        });
        if uses_selector {
            runtime.processor_registry.requeue("Gateway", key).await;
        }
    }
}

/// Kubernetes Controller that spawns independent ResourceControllers for each resource type
///
/// Note: Leader election is handled externally by `KubernetesCenter` (center.rs).
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
    /// Leader handle for gating status writes in all-serve HA mode
    leader_handle: Option<super::leader_election::LeaderHandle>,
    /// Resource kinds to skip (not watch, not register, not probe)
    no_watch_kinds: HashSet<String>,
}

impl KubernetesController {
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
        no_watch_kinds: HashSet<String>,
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
            no_watch_kinds = ?no_watch_kinds,
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
            leader_handle: None,
            no_watch_kinds,
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
    pub async fn run(
        &self,
        shutdown_signal: ShutdownSignal,
        runtime: Arc<ControllerRuntime>,
    ) -> Result<ControllerExitReason> {
        self.run_controllers(shutdown_signal, runtime).await
    }

    /// Phase 1 foundation resource kinds that must be ready before Phase 2 starts.
    ///
    /// These are the resources that Phase 2 resources (Routes, EdgionTls, etc.)
    /// depend on during their init processing:
    /// - GatewayClass: Gateway validation requires GatewayClass reference
    /// - Gateway: Route lookup_gateway() needs Gateway in cache
    /// - Secret: Gateway TLS listeners and Route auth need Secret
    /// - ReferenceGrant: Cross-namespace reference validation
    /// - Service: Route backend references need Service info
    fn phase1_kinds(&self) -> Vec<&'static str> {
        let mut kinds = vec![
            "GatewayClass",
            "Gateway",
            "Secret",
            "ConfigMap",
            "ReferenceGrant",
            "Service",
            "EndpointSlice",
        ];
        kinds.retain(|k| !self.no_watch_kinds.contains(*k));
        kinds
    }

    /// All resource kinds that should register processors during controller startup.
    ///
    /// This includes both Phase 1 foundation resources and Phase 2 dependent
    /// resources. The namespace watcher is intentionally excluded because it
    /// does not participate in PROCESSOR_REGISTRY readiness gating.
    /// Kinds listed in `no_watch_kinds` are excluded.
    pub(crate) fn all_processor_kinds(&self) -> Vec<&'static str> {
        let mut kinds = self.phase1_kinds();
        kinds.extend(
            [
                "HTTPRoute",
                "GRPCRoute",
                "TCPRoute",
                "UDPRoute",
                "TLSRoute",
                "EdgionTls",
                "BackendTLSPolicy",
                "EdgionPlugins",
                "EdgionStreamPlugins",
                "EdgionConfigData",
                "LinkSys",
                "EdgionAIService",
                "EdgionAcme",
                "EdgionBackendTrafficPolicy",
                "EdgionGatewayConfig",
            ]
            .into_iter()
            .filter(|k| !self.no_watch_kinds.contains(*k)),
        );
        kinds
    }

    /// Check if a resource kind should be watched
    fn should_watch(&self, kind: &str) -> bool {
        !self.no_watch_kinds.contains(kind)
    }

    /// Internal method to run all controllers with phased initialization.
    ///
    /// Phase 1 (Foundation): GatewayClass, Gateway, Secret, ReferenceGrant,
    ///   Service, EndpointSlice, Namespace watcher
    /// Phase 2 (Dependent): Routes, EdgionTls, BackendTLSPolicy, Plugins, etc.
    ///
    /// Phase 2 waits for Phase 1 to complete init before starting, so that
    /// dependent resources find their references already in cache on first
    /// processing — reducing post-init revalidation and duplicate status writes.
    ///
    /// Returns when shutdown is triggered or a relink signal is received.
    #[allow(clippy::vec_init_then_push)]
    async fn run_controllers(
        &self,
        mut shutdown_signal: ShutdownSignal,
        runtime: Arc<ControllerRuntime>,
    ) -> Result<ControllerExitReason> {
        tracing::info!(
            component = "k8s_controller",
            "Starting Kubernetes controller - spawning ResourceControllers with phased init"
        );

        // Create relink signal channel
        let (relink_tx, mut relink_rx) = mpsc::channel::<RelinkReason>(10);

        // Create SpawnContext
        let ctx = SpawnContext {
            watcher_config: self.watcher_config(),
            shutdown: shutdown_signal.clone(),
            relink_tx: relink_tx.clone(),
            metadata_filter: self.metadata_filter.clone(),
            leader_handle: self.leader_handle.clone(),
            runtime: runtime.clone(),
        };

        // Resolve every multi-version API before spawning any watcher. Returning
        // after JoinHandles exist would detach the already-started tasks and a
        // lifecycle retry could then create duplicate watchers.
        use super::version_detection::{
            probe_backend_tls_policy_version, probe_reference_grant_version, probe_tls_route_version,
        };
        let reference_grant_version = if self.should_watch("ReferenceGrant") {
            Some(
                probe_reference_grant_version(&self.client)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("ReferenceGrant API disappeared after preflight"))?,
            )
        } else {
            None
        };
        let tls_route_version = if self.should_watch("TLSRoute") {
            Some(
                probe_tls_route_version(&self.client)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("TLSRoute API disappeared after preflight"))?,
            )
        } else {
            None
        };
        let backend_tls_policy_version = if self.should_watch("BackendTLSPolicy") {
            Some(
                probe_backend_tls_policy_version(&self.client)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("BackendTLSPolicy API disappeared after preflight"))?,
            )
        } else {
            None
        };

        let mut h = Vec::new();

        // ==================== Phase 1: Foundation Resources ====================
        // These must be ready before Phase 2 resources start, so that
        // Routes can resolve Gateway/Secret/Service references on first processing.

        tracing::info!(
            component = "k8s_controller",
            "Phase 1: Spawning foundation resource controllers"
        );

        // Cluster-scoped foundation
        if self.should_watch("GatewayClass") {
            h.push(spawn_cluster::<GatewayClass, _>(
                self,
                "GatewayClass",
                GatewayClassHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }

        // Gateway (needs GatewayClass, filter by gateway_class)
        if self.should_watch("Gateway") {
            h.push(spawn::<Gateway, _>(
                self,
                "Gateway",
                GatewayHandler::new(Some(self.gateway_class_name.clone()), self.gateway_address.clone()),
                &ctx,
            ));
        }

        // Secret (TLS certificates, auth credentials)
        if self.should_watch("Secret") {
            h.push(spawn::<Secret, _>(self, "Secret", SecretHandler::new(), &ctx));
        }

        // ConfigMap (CA certificates for frontendValidation and BackendTLSPolicy)
        if self.should_watch("ConfigMap") {
            h.push(spawn::<ConfigMap, _>(self, "ConfigMap", ConfigMapHandler::new(), &ctx));
        }

        // ReferenceGrant (cross-namespace reference validation)
        // Detect v1 vs v1beta1 at runtime without collapsing discovery failures.
        if self.should_watch("ReferenceGrant") {
            use super::version_detection::ReferenceGrantVersion;
            match reference_grant_version.expect("ReferenceGrant version is preflighted when watched") {
                ReferenceGrantVersion::V1 => {
                    h.push(spawn::<ReferenceGrant, _>(
                        self,
                        "ReferenceGrant",
                        ReferenceGrantHandler::new(),
                        &ctx,
                    ));
                }
                ReferenceGrantVersion::V1Beta1 => {
                    h.push(spawn::<ReferenceGrantBeta, _>(
                        self,
                        "ReferenceGrant",
                        ReferenceGrantBetaHandler::new(),
                        &ctx,
                    ));
                }
            }
        }

        // Service (Route backend references)
        if self.should_watch("Service") {
            h.push(spawn::<Service, _>(self, "Service", ServiceHandler::new(), &ctx));
        }

        tracing::info!(
            component = "k8s_controller",
            "Registering EndpointSlice controller (Phase 1)"
        );
        h.push(spawn::<EndpointSlice, _>(
            self,
            "EndpointSlice",
            EndpointSliceHandler::new(),
            &ctx,
        ));

        // Namespace watcher (Gateway Selector namespace policy)
        h.push(spawn_namespace_watcher(
            self.client.clone(),
            watcher::Config::default(),
            ctx.shutdown.clone(),
            ctx.runtime.clone(),
        ));

        let phase1_count = h.len();
        tracing::info!(
            component = "k8s_controller",
            count = phase1_count,
            "Phase 1 foundation controllers spawned, waiting for init completion"
        );

        // Wait for Phase 1 resources to complete their init phase
        const PHASE1_TIMEOUT: Duration = Duration::from_secs(15);
        let phase1_ready = runtime
            .processor_registry
            .wait_kinds_ready(&self.phase1_kinds(), PHASE1_TIMEOUT)
            .await;

        if phase1_ready {
            tracing::info!(
                component = "k8s_controller",
                "Phase 1 complete: all foundation resources ready, starting Phase 2"
            );
        } else {
            tracing::warn!(
                component = "k8s_controller",
                "Phase 1 timeout: starting Phase 2 anyway (fallback to parallel init)"
            );
        }

        // ==================== Phase 2: Dependent Resources ====================
        // These resources depend on Phase 1 resources being in cache.

        tracing::info!(
            component = "k8s_controller",
            "Phase 2: Spawning dependent resource controllers"
        );

        // Route resources
        if self.should_watch("HTTPRoute") {
            h.push(spawn::<HTTPRoute, _>(
                self,
                "HTTPRoute",
                HttpRouteHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("GRPCRoute") {
            h.push(spawn::<GRPCRoute, _>(
                self,
                "GRPCRoute",
                GrpcRouteHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("TCPRoute") {
            h.push(spawn::<TCPRoute, _>(
                self,
                "TCPRoute",
                TcpRouteHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("UDPRoute") {
            h.push(spawn::<UDPRoute, _>(
                self,
                "UDPRoute",
                UdpRouteHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("TLSRoute") {
            use super::version_detection::TLSRouteVersion;
            match tls_route_version.expect("TLSRoute version is preflighted when watched") {
                TLSRouteVersion::V1 => {
                    h.push(spawn::<TLSRoute, _>(
                        self,
                        "TLSRoute",
                        TlsRouteHandler::new(self.controller_name.clone()),
                        &ctx,
                    ));
                }
                TLSRouteVersion::V1Alpha3 => {
                    h.push(spawn::<TLSRouteAlpha3, _>(
                        self,
                        "TLSRoute",
                        TlsRouteAlpha3Handler::new(self.controller_name.clone()),
                        &ctx,
                    ));
                }
            }
        }

        // TLS related (depend on Secret being in cache)
        if self.should_watch("EdgionTls") {
            h.push(spawn::<EdgionTls, _>(
                self,
                "EdgionTls",
                EdgionTlsHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("BackendTLSPolicy") {
            use super::version_detection::BackendTLSPolicyVersion;
            match backend_tls_policy_version.expect("BackendTLSPolicy version is preflighted when watched") {
                BackendTLSPolicyVersion::V1 => {
                    h.push(spawn::<BackendTLSPolicy, _>(
                        self,
                        "BackendTLSPolicy",
                        BackendTlsPolicyHandler::new(self.controller_name.clone()),
                        &ctx,
                    ));
                }
                BackendTLSPolicyVersion::V1Alpha3 => {
                    h.push(spawn::<BackendTLSPolicyAlpha3, _>(
                        self,
                        "BackendTLSPolicy",
                        BackendTlsPolicyAlpha3Handler::new(self.controller_name.clone()),
                        &ctx,
                    ));
                }
            }
        }

        // Plugin resources
        if self.should_watch("EdgionPlugins") {
            h.push(spawn::<EdgionPlugins, _>(
                self,
                "EdgionPlugins",
                EdgionPluginsHandler::new(),
                &ctx,
            ));
        }
        if self.should_watch("EdgionStreamPlugins") {
            h.push(spawn::<EdgionStreamPlugins, _>(
                self,
                "EdgionStreamPlugins",
                EdgionStreamPluginsHandler::new(),
                &ctx,
            ));
        }
        if self.should_watch("EdgionConfigData") {
            h.push(spawn::<EdgionConfigData, _>(
                self,
                "EdgionConfigData",
                EdgionConfigDataHandler::new(),
                &ctx,
            ));
        }
        if self.should_watch("LinkSys") {
            h.push(spawn::<LinkSys, _>(self, "LinkSys", LinkSysHandler::new(), &ctx));
        }
        if self.should_watch("EdgionAIService") {
            h.push(spawn::<EdgionAIService, _>(
                self,
                "EdgionAIService",
                EdgionAIServiceHandler::new(),
                &ctx,
            ));
        }

        // ACME
        if self.should_watch("EdgionAcme") {
            h.push(spawn::<EdgionAcme, _>(
                self,
                "EdgionAcme",
                EdgionAcmeHandler::new(),
                &ctx,
            ));
        }

        // Backend traffic policy
        if self.should_watch("EdgionBackendTrafficPolicy") {
            h.push(spawn::<EdgionBackendTrafficPolicy, _>(
                self,
                "EdgionBackendTrafficPolicy",
                EdgionBackendTrafficPolicyHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }

        // Cluster-scoped dependent
        if self.should_watch("EdgionGatewayConfig") {
            h.push(spawn_cluster::<EdgionGatewayConfig, _>(
                self,
                "EdgionGatewayConfig",
                EdgionGatewayConfigHandler::new(),
                &ctx,
            ));
        }

        // Drop our copy of the sender so we can detect when all controllers stop
        drop(relink_tx);

        tracing::info!(
            component = "k8s_controller",
            phase1_count = phase1_count,
            phase2_count = h.len() - phase1_count,
            total = h.len(),
            "All ResourceControllers spawned (Phase 1 + Phase 2)"
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
