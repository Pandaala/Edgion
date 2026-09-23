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
//! │           │     [Same flow + controllerName ownership]                          │
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

use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::KubernetesStatusIdentity;
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
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use super::namespace::NamespaceWatchMode;
use super::resource_controller::{ApiScope, ResourceController};
use super::ShutdownSignal;
use crate::conf_mgr::conf_center::MetadataFilterConfig;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayClassOwnership;
use crate::conf_mgr::sync_runtime::resource_processor::{ProcessorHandler, ResourceProcessor};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::*;
use edgion_resources::ResourceMeta;

// Import handlers from conf_mgr
use crate::conf_mgr::sync_runtime::resource_processor::{
    BackendTlsPolicyHandler, ConfigMapHandler, EdgionAcmeHandler, EdgionBackendHandler,
    EdgionBackendTrafficPolicyHandler, EdgionConfigDataHandler, EdgionGatewayConfigHandler, EdgionPluginsHandler,
    EdgionStreamPluginsHandler, EdgionTlsHandler, EndpointSliceHandler, GatewayClassHandler, GatewayHandler,
    GrpcRouteHandler, HttpRouteHandler, LinkSysHandler, ReferenceGrantBetaHandler, ReferenceGrantHandler,
    SecretHandler, ServiceHandler, TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
};

/// Context for spawn functions
struct SpawnContext {
    watcher_config: watcher::Config,
    shutdown: ShutdownSignal,
    cancel: CancellationToken,
    tracker: TaskTracker,
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
) -> impl std::future::Future<Output = Result<()>> + Send + 'static
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
    .with_epoch(ctx.cancel.clone(), ctx.tracker.clone());

    if let Some(ref lh) = ctx.leader_handle {
        rc = rc.with_leader_handle(lh.clone());
    }

    ctx.tracker.track_future(async move { rc.run_namespaced().await })
}

/// GatewayClass reporting covers all classes with our controllerName, even if
/// operator selectors exclude them from the rest of the deployment's view.
fn cluster_watcher_config(kind: &str, config: &watcher::Config) -> watcher::Config {
    let mut config = config.clone();
    if kind == "GatewayClass" {
        config.label_selector = None;
    }
    config
}

/// Spawn a cluster-scoped ResourceController with the given handler
fn spawn_cluster<K, H>(
    controller: &KubernetesController,
    kind: &'static str,
    handler: H,
    ctx: &SpawnContext,
) -> impl std::future::Future<Output = Result<()>> + Send + 'static
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
        cluster_watcher_config(kind, &ctx.watcher_config),
    )
    .with_shutdown(ctx.shutdown.clone())
    .with_epoch(ctx.cancel.clone(), ctx.tracker.clone());

    if let Some(ref lh) = ctx.leader_handle {
        rc = rc.with_leader_handle(lh.clone());
    }

    ctx.tracker.track_future(async move { rc.run_cluster_scoped().await })
}

/// Spawn a lightweight Namespace label watcher.
///
/// Unlike other resources, Namespace does not go through the ResourceProcessor
/// pipeline. It only populates the NamespaceStore in ControllerRuntime for Selector
/// namespace policy evaluation.
// Keep the independent readiness and epoch ownership signals explicit at this spawn boundary.
#[allow(clippy::too_many_arguments)]
fn spawn_namespace_watcher(
    client: Client,
    watcher_config: watcher::Config,
    shutdown: ShutdownSignal,
    runtime: Arc<ControllerRuntime>,
    cancel: CancellationToken,
    initial_ready: watch::Sender<bool>,
    gateway_ready: watch::Receiver<bool>,
    gateway_enabled: bool,
) -> impl std::future::Future<Output = Result<()>> + Send + 'static {
    let stream = watcher::watcher(kube::Api::<Namespace>::all(client), watcher_config).default_backoff();
    run_namespace_stream(
        stream,
        shutdown,
        runtime,
        cancel,
        initial_ready,
        gateway_ready,
        gateway_enabled,
    )
}

/// One owner serializes Namespace commits and deferred Selector refresh.
async fn run_namespace_stream<W>(
    stream: W,
    mut shutdown: ShutdownSignal,
    runtime: Arc<ControllerRuntime>,
    cancel: CancellationToken,
    initial_ready: watch::Sender<bool>,
    mut gateway_ready: watch::Receiver<bool>,
    gateway_enabled: bool,
) -> Result<()>
where
    W: futures::Stream<Item = watcher::Result<kube::runtime::watcher::Event<Namespace>>>,
{
    use kube::runtime::watcher::Event;
    futures::pin_mut!(stream);
    let mut batch: Option<Vec<Namespace>> = None;
    let mut committed = false;
    let mut pending_refresh = false;
    let processing = async {
        loop {
            if pending_refresh && *gateway_ready.borrow() {
                requeue_selector_gateways(runtime.clone()).await;
                pending_refresh = false;
            }
            tokio::select! {
                changed = gateway_ready.changed(), if gateway_enabled && !*gateway_ready.borrow() => {
                    changed.map_err(|_| anyhow::anyhow!("Gateway readiness sender disappeared"))?;
                }
                event = stream.try_next() => {
                    match event {
                        Ok(Some(Event::Init)) => batch = Some(Vec::new()),
                        Ok(Some(Event::InitApply(ns))) => {
                            let Some(batch) = &mut batch else { anyhow::bail!("Namespace InitApply outside LIST"); };
                            validate_namespace_identity(&ns)?;
                            batch.push(ns);
                        }
                        Ok(Some(Event::InitDone)) => {
                            if let Some(batch) = batch.take() {
                                runtime.namespace_store.replace_all(batch.into_iter()
                                    .filter(|ns| ns.metadata.deletion_timestamp.is_none()).collect());
                                if committed { pending_refresh |= gateway_enabled; }
                                else {
                                    committed = true;
                                    initial_ready.send_replace(true);
                                }
                            }
                        }
                        Ok(Some(Event::Apply(ns))) => {
                            validate_namespace_identity(&ns)?;
                            anyhow::ensure!(committed && batch.is_none(), "Namespace Apply outside WATCH");
                            let changed = if ns.metadata.deletion_timestamp.is_some() {
                                runtime.namespace_store.remove(&ns.name_any())
                            } else { runtime.namespace_store.upsert(ns) };
                            pending_refresh |= changed && gateway_enabled;
                        }
                        Ok(Some(Event::Delete(ns))) => {
                            validate_namespace_identity(&ns)?;
                            anyhow::ensure!(committed && batch.is_none(), "Namespace Delete outside WATCH");
                            pending_refresh |= runtime.namespace_store.remove(&ns.name_any()) && gateway_enabled;
                        }
                        Ok(None) => anyhow::bail!("Required Namespace source ended"),
                        Err(error) => tracing::warn!(component = "namespace_watcher", error = %error,
                            "Watch error; retaining committed Namespace snapshot"),
                    }
                }
            }
        }
        #[allow(unreachable_code)]
        Ok(())
    };
    tokio::select! {
        result = processing => result,
        _ = cancel.cancelled() => Ok(()),
        _ = shutdown.wait() => Ok(()),
    }
}

fn validate_namespace_identity(ns: &Namespace) -> Result<()> {
    anyhow::ensure!(
        ns.metadata.name.as_deref().is_some_and(|name| !name.is_empty()) && ns.metadata.namespace.is_none(),
        "Invalid Namespace source identity"
    );
    Ok(())
}

/// Requeue all Gateways that use Selector namespace policy so they
/// re-evaluate listener namespace constraints.
async fn requeue_selector_gateways(runtime: Arc<ControllerRuntime>) {
    let Some(processor) = runtime.processor_registry.get("Gateway") else {
        return;
    };
    // Iterate cache keys (already filtered by our controllerName via the
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
    cancel: CancellationToken,
    tracker: TaskTracker,
    phases_ready: Option<watch::Sender<bool>>,
}

impl KubernetesController {
    /// Create a new KubernetesController with metadata filter
    ///
    /// Accepts an external Client to enable Client reuse across components.
    #[allow(clippy::too_many_arguments)]
    pub fn with_metadata_filter(
        client: Client,
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
            controller_name = %controller_name,
            gateway_address = ?gateway_address,
            metadata_filter_enabled = true,
            no_watch_kinds = ?no_watch_kinds,
            "Creating Kubernetes controller"
        );

        Ok(Self {
            client,
            controller_name,
            gateway_address,
            watch_mode,
            label_selector,
            metadata_filter: Some(metadata_filter),
            leader_handle: None,
            no_watch_kinds,
            cancel: CancellationToken::new(),
            tracker: TaskTracker::new(),
            phases_ready: None,
        })
    }

    pub(super) fn with_epoch(mut self, cancel: CancellationToken, tracker: TaskTracker) -> Self {
        self.cancel = cancel;
        self.tracker = tracker;
        self
    }

    pub(super) fn with_phases_ready(mut self, ready: watch::Sender<bool>) -> Self {
        self.phases_ready = Some(ready);
        self
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
    /// - Source-local relist recovery and supervised epoch teardown
    pub async fn run(
        &self,
        shutdown_signal: ShutdownSignal,
        runtime: Arc<ControllerRuntime>,
    ) -> Result<ControllerExitReason> {
        runtime.set_status_identity(KubernetesStatusIdentity {
            controller_name: self.controller_name.clone(),
        });
        self.run_controllers(shutdown_signal, runtime).await
    }

    /// Provider kinds that must be ready before Gateway initialization starts.
    ///
    /// These are the resources that Phase 2 resources (Routes, EdgionTls, etc.)
    /// depend on during their init processing:
    /// - GatewayClass: Gateway validation requires GatewayClass reference
    /// - Secret: Gateway TLS listeners and Route auth need Secret
    /// - ReferenceGrant: Cross-namespace reference validation
    /// - Service: Route backend references need Service info
    fn provider_kinds(&self) -> Vec<&'static str> {
        let mut kinds = vec![
            "GatewayClass",
            "Secret",
            "ConfigMap",
            "ReferenceGrant",
            "Service",
            "EndpointSlice",
            "EdgionConfigData",
        ];
        kinds.retain(|k| !self.no_watch_kinds.contains(*k));
        kinds
    }

    /// All resource kinds that should register processors during controller startup.
    ///
    /// This includes providers, Gateway, and dependent
    /// resources. Namespace is not a processor kind; its first-ready gate is
    /// included in the separate phases-ready publication signal.
    /// Kinds listed in `no_watch_kinds` are excluded.
    pub(crate) fn all_processor_kinds(&self) -> Vec<&'static str> {
        let mut kinds = self.provider_kinds();
        if self.should_watch("Gateway") {
            kinds.push("Gateway");
        }
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
                "LinkSys",
                "EdgionBackend",
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
    /// Phase 1 (Providers): GatewayClass, Secret, ConfigMap, ReferenceGrant,
    ///   Service, EndpointSlice, EdgionConfigData, and Namespace watcher
    /// Phase 2: Gateway
    /// Phase 3 (Dependents): Routes, EdgionTls, BackendTLSPolicy, Plugins, etc.
    ///
    /// Each phase waits for the previous one to complete init, so that
    /// dependent resources find their references already in cache on first
    /// processing — reducing post-init revalidation and duplicate status writes.
    ///
    /// Returns on shutdown, epoch cancellation, or any required child failure.
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

        let cancel = self.cancel.child_token();
        let _cancel_on_exit = cancel.clone().drop_guard();
        let (namespace_ready_tx, mut namespace_ready) = watch::channel(false);
        let (gateway_ready_tx, gateway_ready) = watch::channel(false);

        // Create SpawnContext
        let ctx = SpawnContext {
            watcher_config: self.watcher_config(),
            shutdown: shutdown_signal.clone(),
            cancel: cancel.clone(),
            tracker: self.tracker.clone(),
            metadata_filter: self.metadata_filter.clone(),
            leader_handle: self.leader_handle.clone(),
            runtime: runtime.clone(),
        };

        // Resolve every multi-version API before spawning any watcher. Returning
        // before startup ensures an API-resolution failure cannot leave partially
        // registered resource controllers behind.
        use super::version_detection::probe_reference_grant_version;
        let reference_grant_version = if self.should_watch("ReferenceGrant") {
            Some(
                probe_reference_grant_version(&self.client)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("ReferenceGrant API disappeared after preflight"))?,
            )
        } else {
            None
        };
        let mut h = JoinSet::new();

        // ==================== Phase 1: Provider Resources ====================

        tracing::info!(
            component = "k8s_controller",
            "Phase 1: Spawning provider resource controllers"
        );

        // Cluster-scoped foundation
        if self.should_watch("GatewayClass") {
            h.spawn(spawn_cluster::<GatewayClass, _>(
                self,
                "GatewayClass",
                GatewayClassHandler::new(GatewayClassOwnership::new(self.controller_name.clone())),
                &ctx,
            ));
        }

        // Secret (TLS certificates, auth credentials)
        if self.should_watch("Secret") {
            h.spawn(spawn::<Secret, _>(self, "Secret", SecretHandler::new(), &ctx));
        }

        // ConfigMap (CA certificates for frontendValidation and BackendTLSPolicy)
        if self.should_watch("ConfigMap") {
            h.spawn(spawn::<ConfigMap, _>(self, "ConfigMap", ConfigMapHandler::new(), &ctx));
        }

        // ReferenceGrant (cross-namespace reference validation)
        // Detect v1 vs v1beta1 at runtime without collapsing discovery failures.
        if self.should_watch("ReferenceGrant") {
            use super::version_detection::ReferenceGrantVersion;
            match reference_grant_version.expect("ReferenceGrant version is preflighted when watched") {
                ReferenceGrantVersion::V1 => {
                    h.spawn(spawn::<ReferenceGrant, _>(
                        self,
                        "ReferenceGrant",
                        ReferenceGrantHandler::new(),
                        &ctx,
                    ));
                }
                ReferenceGrantVersion::V1Beta1 => {
                    h.spawn(spawn::<ReferenceGrantBeta, _>(
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
            h.spawn(spawn::<Service, _>(self, "Service", ServiceHandler::new(), &ctx));
        }

        tracing::info!(
            component = "k8s_controller",
            "Registering EndpointSlice controller (Phase 1)"
        );
        h.spawn(spawn::<EndpointSlice, _>(
            self,
            "EndpointSlice",
            EndpointSliceHandler::new(),
            &ctx,
        ));

        // Namespace watcher (Gateway Selector namespace policy)
        h.spawn(ctx.tracker.track_future(spawn_namespace_watcher(
            self.client.clone(),
            watcher::Config::default(),
            ctx.shutdown.clone(),
            ctx.runtime.clone(),
            cancel.clone(),
            namespace_ready_tx,
            gateway_ready,
            self.should_watch("Gateway"),
        )));

        // EdgionConfigData supplies referenced inbound PROXY policies and must
        // be complete before any Gateway is resolved.
        if self.should_watch("EdgionConfigData") {
            h.spawn(spawn::<EdgionConfigData, _>(
                self,
                "EdgionConfigData",
                EdgionConfigDataHandler::new(),
                &ctx,
            ));
        }

        let provider_count = h.len();
        tracing::info!(
            component = "k8s_controller",
            count = provider_count,
            "Provider controllers spawned, waiting for init completion"
        );

        const PHASE_TIMEOUT: Duration = Duration::from_secs(15);
        let providers = self.provider_kinds();
        wait_phase(
            &mut h,
            &cancel,
            &mut shutdown_signal,
            async {
                let ready = runtime
                    .processor_registry
                    .wait_kinds_ready(&providers, PHASE_TIMEOUT)
                    .await;
                anyhow::ensure!(ready, "Provider initialization timed out");
                while !*namespace_ready.borrow() {
                    namespace_ready
                        .changed()
                        .await
                        .map_err(|_| anyhow::anyhow!("Namespace readiness sender disappeared"))?;
                }
                Ok(())
            },
            PHASE_TIMEOUT,
        )
        .await?;
        tracing::info!(component = "k8s_controller", "Phase 1 complete: all providers ready");

        // ==================== Phase 2: Gateway ====================
        tracing::info!(component = "k8s_controller", "Phase 2: Spawning Gateway controller");
        if self.should_watch("Gateway") {
            h.spawn(spawn::<Gateway, _>(
                self,
                "Gateway",
                GatewayHandler::new(
                    Some(GatewayClassOwnership::new(self.controller_name.clone())),
                    self.gateway_address.clone(),
                ),
                &ctx,
            ));
        }
        let gateway_count = h.len();
        let gateway_kinds = if self.should_watch("Gateway") {
            vec!["Gateway"]
        } else {
            Vec::new()
        };
        wait_phase(
            &mut h,
            &cancel,
            &mut shutdown_signal,
            async {
                anyhow::ensure!(
                    runtime
                        .processor_registry
                        .wait_kinds_ready(&gateway_kinds, PHASE_TIMEOUT)
                        .await,
                    "Gateway initialization timed out"
                );
                Ok(())
            },
            PHASE_TIMEOUT,
        )
        .await?;
        gateway_ready_tx.send_replace(true);
        tracing::info!(component = "k8s_controller", "Phase 2 complete: Gateway ready");

        // ==================== Phase 3: Dependent Resources ====================

        tracing::info!(
            component = "k8s_controller",
            "Phase 3: Spawning dependent resource controllers"
        );

        // Route resources
        if self.should_watch("HTTPRoute") {
            h.spawn(spawn::<HTTPRoute, _>(
                self,
                "HTTPRoute",
                HttpRouteHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("GRPCRoute") {
            h.spawn(spawn::<GRPCRoute, _>(
                self,
                "GRPCRoute",
                GrpcRouteHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("TCPRoute") {
            h.spawn(spawn::<TCPRoute, _>(
                self,
                "TCPRoute",
                TcpRouteHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("UDPRoute") {
            h.spawn(spawn::<UDPRoute, _>(
                self,
                "UDPRoute",
                UdpRouteHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("TLSRoute") {
            h.spawn(spawn::<TLSRoute, _>(
                self,
                "TLSRoute",
                TlsRouteHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }

        // TLS related (depend on Secret being in cache)
        if self.should_watch("EdgionTls") {
            h.spawn(spawn::<EdgionTls, _>(
                self,
                "EdgionTls",
                EdgionTlsHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }
        if self.should_watch("BackendTLSPolicy") {
            h.spawn(spawn::<BackendTLSPolicy, _>(
                self,
                "BackendTLSPolicy",
                BackendTlsPolicyHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }

        // Plugin resources
        if self.should_watch("EdgionPlugins") {
            h.spawn(spawn::<EdgionPlugins, _>(
                self,
                "EdgionPlugins",
                EdgionPluginsHandler::new(),
                &ctx,
            ));
        }
        if self.should_watch("EdgionStreamPlugins") {
            h.spawn(spawn::<EdgionStreamPlugins, _>(
                self,
                "EdgionStreamPlugins",
                EdgionStreamPluginsHandler::new(),
                &ctx,
            ));
        }
        if self.should_watch("LinkSys") {
            h.spawn(spawn::<LinkSys, _>(self, "LinkSys", LinkSysHandler::new(), &ctx));
        }
        if self.should_watch("EdgionBackend") {
            h.spawn(spawn::<EdgionBackend, _>(
                self,
                "EdgionBackend",
                EdgionBackendHandler::new(),
                &ctx,
            ));
        }

        // ACME
        if self.should_watch("EdgionAcme") {
            h.spawn(spawn::<EdgionAcme, _>(
                self,
                "EdgionAcme",
                EdgionAcmeHandler::new(),
                &ctx,
            ));
        }

        // Backend traffic policy
        if self.should_watch("EdgionBackendTrafficPolicy") {
            h.spawn(spawn::<EdgionBackendTrafficPolicy, _>(
                self,
                "EdgionBackendTrafficPolicy",
                EdgionBackendTrafficPolicyHandler::new(self.controller_name.clone()),
                &ctx,
            ));
        }

        // Cluster-scoped dependent
        if self.should_watch("EdgionGatewayConfig") {
            h.spawn(spawn_cluster::<EdgionGatewayConfig, _>(
                self,
                "EdgionGatewayConfig",
                EdgionGatewayConfigHandler::new(),
                &ctx,
            ));
        }

        tracing::info!(
            component = "k8s_controller",
            provider_count = provider_count,
            gateway_count = gateway_count - provider_count,
            dependent_count = h.len() - gateway_count,
            total = h.len(),
            "All ResourceControllers spawned (providers + Gateway + dependents)"
        );

        if let Some(ready) = &self.phases_ready {
            ready.send_replace(true);
        }
        let exit_reason = tokio::select! {
            _ = shutdown_signal.wait() => ControllerExitReason::Shutdown,
            _ = cancel.cancelled() => ControllerExitReason::Shutdown,
            result = h.join_next() => {
                tracing::error!(component = "k8s_controller", result = ?result, "Required resource controller stopped");
                ControllerExitReason::AllControllersStopped
            }
        };
        cancel.cancel();
        if tokio::time::timeout(Duration::from_secs(5), async { while h.join_next().await.is_some() {} })
            .await
            .is_err()
        {
            h.shutdown().await;
        }

        Ok(exit_reason)
    }
}

/// Reason for controller exit
#[derive(Debug)]
pub enum ControllerExitReason {
    /// Shutdown was requested
    Shutdown,
    /// Administrative reload was requested.
    ReloadRequested,
    /// A required controller stopped unexpectedly
    AllControllersStopped,
    /// Lost leadership
    LostLeadership,
}

/// Phase waits observe every required child, including a child that fails before readiness.
async fn wait_phase<F>(
    children: &mut JoinSet<Result<()>>,
    cancel: &CancellationToken,
    shutdown: &mut ShutdownSignal,
    ready: F,
    deadline: Duration,
) -> Result<()>
where
    F: std::future::Future<Output = Result<()>>,
{
    tokio::select! {
        result = tokio::time::timeout(deadline, ready) => result.map_err(|_| anyhow::anyhow!("Resource phase timed out"))?,
        result = children.join_next() => anyhow::bail!("Required child stopped during initialization: {:?}", result),
        _ = cancel.cancelled() => anyhow::bail!("Resource epoch cancelled during initialization"),
        _ = shutdown.wait() => anyhow::bail!("Shutdown during initialization"),
    }
}

#[cfg(test)]
mod source_lifecycle_tests {
    use super::super::ShutdownController;
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::raw_store::LocalRawStore;
    use futures::FutureExt;
    use kube::runtime::watcher::Event;

    #[test]
    fn gateway_class_reporting_bypasses_labels_without_changing_other_watches() {
        let config = watcher::Config::default().labels("team=blue");
        assert!(cluster_watcher_config("GatewayClass", &config).label_selector.is_none());
        assert_eq!(
            cluster_watcher_config("EdgionGatewayConfig", &config).label_selector,
            Some("team=blue".into())
        );
        assert_eq!(config.label_selector, Some("team=blue".into()));
    }

    struct GatewayFixtureHandler;
    #[async_trait::async_trait]
    impl ProcessorHandler<Gateway> for GatewayFixtureHandler {}

    fn ns(name: &str) -> Namespace {
        let mut ns = Namespace::default();
        ns.metadata.name = Some(name.into());
        ns
    }

    async fn eventually(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !predicate() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn namespace_relist_and_incremental_changes_defer_refresh_until_gateway_bootstrap() {
        let runtime = Arc::new(ControllerRuntime::new());
        let processor = Arc::new(ResourceProcessor::new(
            "Gateway",
            10,
            Arc::new(GatewayFixtureHandler),
            runtime.clone(),
        ));
        runtime.processor_registry.register(processor.clone());
        let gateway: Gateway = serde_json::from_value(serde_json::json!({
            "metadata": {"name":"gateway", "namespace":"a"},
            "spec": {"gatewayClassName":"edgion", "listeners":[{"name":"http", "port":80,"protocol":"HTTP",
                "allowedRoutes":{"namespaces":{"from":"Selector","selector":{"matchLabels":{"team":"a"}}}}}]}
        }))
        .unwrap();
        let raw = Arc::new(LocalRawStore::new());
        raw.upsert("a/gateway".into(), Arc::new(gateway.clone()));
        processor.set_raw_store(raw);
        let (shutdown, signal) = ShutdownController::new();
        let cancel = CancellationToken::new();
        let (ready_tx, ready_rx) = watch::channel(false);
        let (gateway_tx, gateway_rx) = watch::channel(false);
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let stream = futures::stream::unfold(rx, |mut rx| async { rx.recv().await.map(|event| (event, rx)) });
        let task = tokio::spawn(run_namespace_stream(
            stream,
            signal,
            runtime.clone(),
            cancel.clone(),
            ready_tx,
            gateway_rx,
            true,
        ));
        for event in [Event::Init, Event::InitApply(ns("a")), Event::InitDone] {
            tx.send(Ok(event)).await.unwrap();
        }
        eventually(|| *ready_rx.borrow()).await;
        assert!(runtime.namespace_store.get("a").is_some());
        // Relist retains the previous committed map until completion, then removes terminating namespaces.
        let mut terminating = ns("a");
        terminating.metadata.deletion_timestamp =
            Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(chrono::Utc::now()));
        for event in [
            Event::Init,
            Event::InitApply(terminating),
            Event::InitDone,
            Event::InitDone,
            Event::Apply(ns("b")),
        ] {
            tx.send(Ok(event)).await.unwrap();
        }
        eventually(|| runtime.namespace_store.get("a").is_none() && runtime.namespace_store.get("b").is_some()).await;
        assert!(processor.workqueue().dequeue().now_or_never().is_none());
        // Publish Gateway only after the Namespace changes: an early enumeration would miss it.
        processor.save(gateway, None);
        processor.on_init_done();
        gateway_tx.send_replace(true);
        let item = tokio::time::timeout(Duration::from_secs(5), processor.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.key, "a/gateway");
        tx.send(Ok(Event::Delete(ns("b")))).await.unwrap();
        let item = tokio::time::timeout(Duration::from_secs(5), processor.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.key, "a/gateway");
        assert!(runtime.namespace_store.get("b").is_none());
        shutdown.shutdown();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn namespace_disabled_gateway_needs_no_ready_signal_and_rejects_bad_runtime_identity() {
        for bad_delete in [false, true] {
            let runtime = Arc::new(ControllerRuntime::new());
            let (_shutdown, signal) = ShutdownController::new();
            let (ready_tx, ready_rx) = watch::channel(false);
            let (_gateway_tx, gateway_rx) = watch::channel(false);
            let mut invalid = ns("bad");
            invalid.metadata.namespace = Some("wrong".into());
            let events = vec![
                Ok(Event::Init),
                Ok(Event::InitApply(ns("a"))),
                Ok(Event::InitDone),
                Ok(Event::Apply(ns("b"))),
                Ok(if bad_delete {
                    Event::Delete(invalid)
                } else {
                    Event::Apply(invalid)
                }),
            ];
            let result = run_namespace_stream(
                futures::stream::iter(events),
                signal,
                runtime.clone(),
                CancellationToken::new(),
                ready_tx,
                gateway_rx,
                false,
            )
            .await;
            assert!(result.is_err());
            assert!(*ready_rx.borrow());
            assert!(runtime.namespace_store.get("b").is_some());
            assert!(runtime.namespace_store.get("bad").is_none());
        }
    }

    #[tokio::test]
    async fn phase_wait_observes_required_child_failure_without_waiting_for_timeout() {
        let mut children = JoinSet::new();
        children.spawn(async { Err(anyhow::anyhow!("source EOF")) });
        let (_shutdown, mut signal) = ShutdownController::new();
        let result = wait_phase(
            &mut children,
            &CancellationToken::new(),
            &mut signal,
            futures::future::pending(),
            Duration::from_secs(60),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("Required child"));
    }
}
