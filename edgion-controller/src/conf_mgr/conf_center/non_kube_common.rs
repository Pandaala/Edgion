//! Shared infrastructure for non-Kubernetes configuration center backends
//! (FileSystem and Etcd modes).

use std::fmt::Debug;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use k8s_openapi::api::core::v1::{ConfigMap, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;
use parking_lot::{Mutex, RwLock};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::conf_mgr::conf_center::etcd::leadership_gate::LeaderHandle;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayClassOwnership;
use crate::conf_mgr::sync_runtime::resource_processor::{
    run_post_cache_ready_revalidations, BackendTlsPolicyHandler, ConfigMapHandler, EdgionAcmeHandler,
    EdgionBackendHandler, EdgionBackendTrafficPolicyHandler, EdgionConfigDataHandler, EdgionGatewayConfigHandler,
    EdgionPluginsHandler, EdgionStreamPluginsHandler, EdgionTlsHandler, EndpointSliceHandler, GatewayClassHandler,
    GatewayHandler, GrpcRouteHandler, HttpRouteHandler, LinkSysHandler, ProcessorHandler, ReferenceGrantHandler,
    ResourceProcessor, SecretHandler, ServiceHandler, TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
};
use crate::conf_mgr::sync_runtime::{ShutdownHandle, ShutdownSignal};
use crate::conf_mgr::ControllerRuntime;
use crate::conf_sync::conf_server::ConfigSyncServer;
use edgion_resources::prelude_resources::*;
use edgion_resources::ResourceMeta;

/// Cancellation and descendant ownership for one local-backend iteration.
/// Register tasks synchronously before they run, including tasks spawned by
/// other tracked tasks. The center joins the top-level spawner before draining.
#[derive(Clone, Default)]
pub struct LocalTaskScope {
    pub(super) cancel: CancellationToken,
    pub(super) tracker: TaskTracker,
}

impl LocalTaskScope {
    pub(super) fn spawn(
        &self,
        future: impl std::future::Future<Output = Result<()>> + Send + 'static,
    ) -> JoinHandle<Result<()>> {
        let cancel = self.cancel.clone();
        self.tracker.spawn(async move {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Ok(()),
                result = future => result,
            }
        })
    }
}

/// Shared state for non-Kubernetes configuration center backends.
///
/// Holds the lifecycle handles and shared runtime that are identical between
/// `FileSystemCenter` and `EtcdCenter`. Each backend embeds this as `base`.
pub struct CenterBase {
    config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>,
    shutdown_handle: Mutex<Option<ShutdownHandle>>,
    controller_handle: Mutex<Option<JoinHandle<()>>>,
    runtime: Arc<ControllerRuntime>,
    /// Single startup bound (`cache_ready_deadline_secs`); phase waits are unbounded.
    ready_deadline: Duration,
}

impl CenterBase {
    /// Create a new `CenterBase` with all handles initialised to `None`.
    pub fn new(ready_deadline: Duration) -> Self {
        let runtime = Arc::new(ControllerRuntime::new());
        runtime.set_managed_credentials_supported(false);
        // single_publisher stays true (ControllerRuntime::new default): persist=false is allowed on FS/etcd.
        Self {
            config_sync_server: RwLock::new(None),
            shutdown_handle: Mutex::new(None),
            controller_handle: Mutex::new(None),
            runtime,
            ready_deadline,
        }
    }

    // ==================== Private helper methods ====================

    fn set_config_sync_server(&self, server: Option<Arc<ConfigSyncServer>>, component: &str) {
        let mut sync_server = self.config_sync_server.write();
        let was_ready = sync_server.is_some();
        let is_ready = server.is_some();
        *sync_server = server;

        if was_ready != is_ready {
            tracing::info!(
                component = component,
                was_ready = was_ready,
                is_ready = is_ready,
                "ConfigSyncServer state changed"
            );
        }
    }

    fn set_shutdown_handle(&self, handle: ShutdownHandle) {
        *self.shutdown_handle.lock() = Some(handle);
    }

    fn set_controller_handle(&self, handle: JoinHandle<()>) {
        *self.controller_handle.lock() = Some(handle);
    }

    async fn stop_iteration(&self, scope: &LocalTaskScope, component: &str) {
        let old_css = self.config_sync_server();
        self.set_config_sync_server(None, component);
        scope.cancel.cancel();
        let handle = self.controller_handle.lock().take();
        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }
        // Closing does not prevent registration. Joining the root spawner and
        // tracking every child before it runs prevents an empty-set race.
        scope.tracker.close();
        scope.tracker.wait().await;
        if let Some(css) = old_css {
            css.clear_all();
        }
        self.runtime.reset();
    }

    async fn wait_registry_ready(&self, component: &str) -> bool {
        let registry = &self.runtime.processor_registry;
        let timeout = self.ready_deadline;
        let timeout_secs = timeout.as_secs();
        let start = std::time::Instant::now();
        let notify = registry.ready_notify.clone();

        // Phase A: wait for all processors to be registered (Phase 1 + Phase 2).
        while !registry.is_registration_complete() {
            let elapsed = Instant::now().saturating_duration_since(start);
            if elapsed >= timeout {
                tracing::warn!(
                    component = component,
                    timeout_secs = timeout_secs,
                    "Timeout waiting for processor registration to complete"
                );
                return false;
            }
            let remaining = timeout - elapsed;
            let _ = tokio::time::timeout(remaining.min(Duration::from_secs(1)), notify.notified()).await;
        }

        // Phase B: wait for all registered processors to become ready.
        let remaining = timeout.saturating_sub(Instant::now().saturating_duration_since(start));
        let all_kinds = registry.all_kinds();
        let ready = registry.wait_kinds_ready(&all_kinds, remaining).await;
        if !ready {
            tracing::error!(
                component = component,
                timeout_secs = timeout_secs,
                not_ready = ?registry.not_ready_kinds(),
                "Timeout waiting for all processors"
            );
        }
        ready
    }

    // ==================== Public proxy methods ====================

    /// Check whether this center is ready to serve requests.
    pub fn is_ready(&self) -> bool {
        self.runtime.processor_registry.is_all_ready() && self.config_sync_server.read().is_some()
    }

    /// Get the `ConfigSyncServer` (returns `None` if not yet ready).
    pub fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        self.config_sync_server.read().clone()
    }

    /// Get the shared `ControllerRuntime`.
    pub fn runtime(&self) -> Arc<ControllerRuntime> {
        self.runtime.clone()
    }

    // ==================== Lifecycle ====================

    /// Run the shared controller lifecycle loop.
    ///
    /// # Parameters
    ///
    /// - `shutdown_handle`: The top-level shutdown handle for this center instance.
    /// - `component`: A `'static` string used as the `component` field in every log line.
    /// - `make_handle`: A synchronous closure that spawns the backend-specific controller
    ///   and returns its `JoinHandle`. Receives the iteration-scoped shutdown signal,
    ///   the shared runtime, a oneshot sender for fatal errors, and the task scope
    ///   that must own every resource-controller and worker descendant.
    pub async fn run_lifecycle(
        &self,
        shutdown_handle: ShutdownHandle,
        component: &'static str,
        make_handle: impl Fn(
            ShutdownSignal,
            Arc<ControllerRuntime>,
            oneshot::Sender<String>,
            LocalTaskScope,
        ) -> JoinHandle<()>,
    ) -> Result<()> {
        self.run_lifecycle_gated(shutdown_handle, component, None, make_handle)
            .await
    }

    /// Run the shared controller lifecycle loop with an optional leadership gate.
    ///
    /// This is the gated variant of [`CenterBase::run_lifecycle`]. When
    /// `leadership` is `Some`, each iteration blocks at the top of the loop until
    /// the handle reports leadership, and a mid-term loss of leadership tears the
    /// iteration down and re-parks at the gate. When `leadership` is `None` the
    /// gate is a no-op — the unconditional-leader path used by FileSystem mode
    /// (K8s mode never calls this path at all). See spec §5.
    ///
    /// # Parameters
    ///
    /// - `shutdown_handle`: The top-level shutdown handle for this center instance.
    /// - `component`: A `'static` string used as the `component` field in every log line.
    /// - `leadership`: Optional leadership handle. `None` = unconditional leader.
    /// - `make_handle`: A synchronous closure that spawns the backend-specific controller
    ///   and returns its `JoinHandle`. Receives the iteration-scoped shutdown signal,
    ///   the shared runtime, a oneshot sender for fatal errors, and the task scope
    ///   that must own every resource-controller and worker descendant.
    pub async fn run_lifecycle_gated(
        &self,
        shutdown_handle: ShutdownHandle,
        component: &'static str,
        leadership: Option<LeaderHandle>,
        make_handle: impl Fn(
            ShutdownSignal,
            Arc<ControllerRuntime>,
            oneshot::Sender<String>,
            LocalTaskScope,
        ) -> JoinHandle<()>,
    ) -> Result<()> {
        self.set_shutdown_handle(shutdown_handle.clone());

        loop {
            if shutdown_handle.is_shutdown() {
                return Ok(());
            }

            // 0. Leadership gate (spec §5). `None` ⇒ unconditional leader
            // (FileSystem mode; K8s never calls this path at all). When `Some`,
            // park here until this node is the leader, letting shutdown break out.
            if let Some(ref lh) = leadership {
                let mut gate_shutdown = shutdown_handle.signal();
                loop {
                    if gate_shutdown.is_shutdown() {
                        return Ok(());
                    }
                    if lh.is_leader() {
                        break;
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                        _ = gate_shutdown.wait() => { return Ok(()); }
                    }
                }
            }

            // Iteration-specific shutdown and descendant ownership.
            let iteration_shutdown = ShutdownHandle::new();
            let scope = LocalTaskScope::default();

            // Backend failures share the same teardown barrier.
            let (error_tx, mut error_rx) = oneshot::channel::<String>();

            // Spawn the backend-specific controller.
            let handle = make_handle(
                iteration_shutdown.signal(),
                self.runtime.clone(),
                error_tx,
                scope.clone(),
            );
            self.set_controller_handle(handle);

            // Initialization is part of the iteration: shutdown, leadership
            // loss, and backend failure can interrupt it as well as serving.
            let mut global_shutdown = shutdown_handle.signal();
            let mut initialized = false;
            let initialize_and_serve = async {
                if !self.wait_registry_ready(component).await {
                    return LoopAction::InitializationFailed;
                }
                run_post_cache_ready_revalidations(self.runtime.clone()).await;
                let config_sync_server = Arc::new(ConfigSyncServer::new());
                let no_sync_kinds = crate::config::get_no_sync_kinds();
                let no_sync_refs: Vec<&str> = no_sync_kinds.iter().map(|s| s.as_str()).collect();
                config_sync_server.register_all(self.runtime.processor_registry.all_watch_objs(&no_sync_refs));
                self.set_config_sync_server(Some(config_sync_server), component);
                initialized = true;
                tracing::info!(
                    component = component,
                    "ConfigSyncServer is ready, gRPC services can process requests"
                );
                if leadership.as_ref().is_none_or(|leader| leader.is_leader()) {
                    crate::services::credential_manager::start_memory_credential_manager_service(self.runtime.clone());
                }
                std::future::pending::<LoopAction>().await
            };
            let action = tokio::select! {
                biased;
                _ = global_shutdown.wait() => LoopAction::Shutdown,
                _ = leadership_lost_wait(&leadership) => LoopAction::LeadershipLost,
                result = &mut error_rx => {
                    match result {
                        Ok(msg) => LoopAction::Error(msg),
                        Err(_) if global_shutdown.is_shutdown() => LoopAction::Shutdown,
                        Err(_) => LoopAction::Error("Backend controller error channel closed unexpectedly".to_string()),
                    }
                },
                action = initialize_and_serve => action,
            };

            // Stop the controller for this iteration.
            iteration_shutdown.shutdown();
            crate::services::credential_manager::stop_credential_manager_service();

            // No task from this iteration may mutate the runtime after reset.
            self.stop_iteration(&scope, component).await;
            if shutdown_handle.is_shutdown() {
                return Ok(());
            }

            // Handle the selected action only after shared state is safe to reuse.
            match action {
                LoopAction::Shutdown => {
                    tracing::info!(component = component, "Normal shutdown, lifecycle completed");
                    return Ok(());
                }
                LoopAction::InitializationFailed => {
                    return Err(anyhow::anyhow!(
                        "resource controller initialization did not complete before the readiness deadline"
                    ))
                }
                LoopAction::LeadershipLost => {
                    // Lost leadership mid-term. The teardown above already ran;
                    // loop back to the gate and park until leadership is
                    // regained. Do NOT release leadership here — the election
                    // supervisor owns demotion (spec §5.1a).
                    tracing::warn!(
                        component = component,
                        "Lost leadership, tearing down and re-parking at the gate"
                    );
                    continue;
                }
                LoopAction::Error(msg) => {
                    if !initialized {
                        return Err(anyhow::anyhow!("resource controller initialization failed: {msg}"));
                    }
                    tracing::error!(
                        component = component,
                        error = %msg,
                        "Controller stopped with error, waiting for shutdown"
                    );
                    shutdown_handle.signal().wait().await;
                    return Ok(());
                }
            }
        }
    }
}

impl Default for CenterBase {
    fn default() -> Self {
        Self::new(Duration::from_secs(
            crate::config::conf_center::readiness::DEFAULT_CACHE_READY_DEADLINE_SECS,
        ))
    }
}

/// Action returned by the `run_lifecycle` select loop.
enum LoopAction {
    Shutdown,
    InitializationFailed,
    Error(String),
    LeadershipLost,
}

/// Resolve when leadership is lost.
///
/// With `None` leadership (FileSystem / unconditional-leader path) this never
/// resolves, so the corresponding `select!` arm is inert. With `Some`, it polls
/// `is_leader()` and returns as soon as leadership drops.
async fn leadership_lost_wait(leadership: &Option<LeaderHandle>) {
    match leadership {
        None => std::future::pending::<()>().await,
        Some(lh) => loop {
            if !lh.is_leader() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        },
    }
}

// ============================================================================
// LocalControllerBackend — shared abstraction for FileSystem and Etcd backends
// ============================================================================

/// A backend that can subscribe to resource-kind events and launch per-kind
/// resource controllers.
///
/// Implemented by `EtcdControllerBackend` and `FileSystemControllerBackend`
/// to allow [`spawn_all_controllers`] to work with either backend without
/// duplicating the 20-resource spawn list.
#[async_trait]
pub trait LocalControllerBackend: Send + Sync {
    /// The event-receiver type produced by [`LocalControllerBackend::subscribe`].
    ///
    /// Different for each backend:
    /// - Etcd: `mpsc::Receiver<EtcdEvent>`
    /// - FileSystem: `mpsc::Receiver<FileSystemEvent<String>>`
    type EventReceiver: Send + 'static;

    /// Subscribe to events for a resource kind, returning a channel receiver.
    ///
    /// Must be called before the watcher's `dispatch_init` starts sending events.
    async fn subscribe(&self, kind: &'static str) -> Self::EventReceiver;

    /// Take one full snapshot of backend state (etcd: single-revision prefix
    /// list; FS: directory scan + atomic registry swap). Stores only — emits no
    /// events. Called once before any `dispatch_init`.
    async fn prime(&self) -> Result<()>;

    /// Dispatch `Init` / `InitApply` / `InitDone` to exactly these
    /// already-subscribed kinds, from the snapshot captured by `prime`.
    async fn dispatch_init(&self, kinds: &[&'static str]) -> Result<()>;

    /// Construct and spawn the per-kind resource controller.
    ///
    /// The handler is already embedded in `processor`; `K` determines the
    /// concrete controller type used by each backend.
    fn launch<K>(
        &self,
        kind: &'static str,
        processor: Arc<ResourceProcessor<K>>,
        event_rx: Self::EventReceiver,
        shutdown_signal: ShutdownSignal,
    ) -> JoinHandle<Result<()>>
    where
        K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static;
}

/// Create a `ResourceProcessor`, register it, subscribe to the backend, and
/// launch the per-kind controller — all in one step.
pub async fn spawn<K, H, B>(
    kind: &'static str,
    handler: H,
    backend: &B,
    shutdown_signal: ShutdownSignal,
    runtime: Arc<ControllerRuntime>,
) -> JoinHandle<Result<()>>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    H: ProcessorHandler<K> + 'static,
    B: LocalControllerBackend,
{
    let capacity = crate::config::get_cache_capacity(kind);
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        capacity,
        Arc::new(handler),
        runtime.clone(),
    ));

    runtime.processor_registry.register(processor.clone());

    let event_rx = backend.subscribe(kind).await;

    backend.launch::<K>(kind, processor, event_rx, shutdown_signal)
}

/// Spawn all resource controllers in provider, Gateway, then dependent phases.
///
/// This is the single authoritative resource list shared by both the Etcd and
/// FileSystem controller implementations.  Editing this function is the only
/// place needed when adding or removing a resource type from either backend.
///
/// # Phase ordering
///
/// - **Providers**: GatewayClass, Secret, ConfigMap, ReferenceGrant, Service,
///   EndpointSlice, and EdgionConfigData.
/// - **Gateway**: starts only after every provider is ready.
/// - **Dependents**: start only after Gateway is ready.
pub async fn spawn_all_controllers<B: LocalControllerBackend>(
    backend: &B,
    shutdown_signal: ShutdownSignal,
    runtime: Arc<ControllerRuntime>,
    component: &'static str,
    controller_name: &str,
) -> Result<Vec<JoinHandle<Result<()>>>> {
    let mut handles = Vec::new();
    // Collects the kind name of every spawned controller, in spawn order.
    // Sliced at phase boundaries below.
    let mut spawned_kinds: Vec<&'static str> = Vec::new();

    // Helper macro: reduces the per-resource boilerplate inside this function.
    macro_rules! do_spawn {
        ($K:ty, $handler:expr) => {
            spawned_kinds.push(stringify!($K));
            handles.push(
                spawn::<$K, _, B>(
                    stringify!($K),
                    $handler,
                    backend,
                    shutdown_signal.clone(),
                    runtime.clone(),
                )
                .await,
            );
        };
    }

    // Snapshot backend state once, up front, before any dispatch.
    backend.prime().await?;

    // ==================== Phase 1: Provider Resources ====================
    tracing::info!(component = component, "Phase 1: Spawning provider resource controllers");

    for_each_provider_core_resource!(do_spawn, controller_name);

    tracing::info!(component = component, "Registering EndpointSlice provider controller");
    do_spawn!(EndpointSlice, EndpointSliceHandler::new());

    let provider_count = handles.len();
    let provider_kinds: Vec<&'static str> = spawned_kinds[..provider_count].to_vec();
    tracing::info!(
        component = component,
        count = provider_count,
        "Provider controllers spawned, dispatching init"
    );

    if let Err(error) = backend.dispatch_init(&provider_kinds).await {
        for handle in &handles {
            handle.abort();
        }
        return Err(error);
    }

    // Phase waits are unbounded: the center's `cache_ready_deadline_secs`
    // readiness wait is the single startup bound and tears this task down.
    runtime
        .processor_registry
        .wait_kinds_ready_unbounded(&provider_kinds)
        .await;
    tracing::info!(component = component, "Phase 1 complete: all providers ready");

    // ==================== Phase 2: Gateway ====================
    tracing::info!(component = component, "Phase 2: Spawning Gateway controller");
    do_spawn!(Gateway, GatewayHandler::new(None, None));
    let gateway_count = handles.len();
    let gateway_kinds: Vec<&'static str> = spawned_kinds[provider_count..gateway_count].to_vec();
    if let Err(error) = backend.dispatch_init(&gateway_kinds).await {
        for handle in &handles {
            handle.abort();
        }
        return Err(error);
    }
    runtime
        .processor_registry
        .wait_kinds_ready_unbounded(&gateway_kinds)
        .await;
    tracing::info!(component = component, "Phase 2 complete: Gateway ready");

    // ==================== Phase 3: Dependent Resources ====================
    tracing::info!(
        component = component,
        "Phase 3: Spawning dependent resource controllers"
    );
    for_each_dependent_resource!(do_spawn, controller_name);

    let dependent_kinds: Vec<&'static str> = spawned_kinds[gateway_count..].to_vec();
    if let Err(error) = backend.dispatch_init(&dependent_kinds).await {
        for handle in &handles {
            handle.abort();
        }
        return Err(error);
    }

    tracing::info!(
        component = component,
        provider_count = provider_count,
        gateway_count = gateway_count - provider_count,
        dependent_count = handles.len() - gateway_count,
        total = handles.len(),
        "All resource controllers spawned"
    );

    runtime.processor_registry.mark_registration_complete();

    Ok(handles)
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_mgr/conf_center/non_kube_common_test.rs"]
mod tests;
