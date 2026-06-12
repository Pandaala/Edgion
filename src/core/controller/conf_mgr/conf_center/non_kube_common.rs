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
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::core::controller::conf_mgr::sync_runtime::metrics::reload_metrics;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    run_post_cache_ready_revalidations, BackendTlsPolicyHandler, ConfigMapHandler, EdgionAcmeHandler,
    EdgionGatewayConfigHandler, EdgionPluginsHandler, EdgionStreamPluginsHandler, EdgionTlsHandler,
    EndpointSliceHandler, GatewayClassHandler, GatewayHandler, GrpcRouteHandler, HttpRouteHandler, LinkSysHandler,
    PluginMetadataHandler, ProcessorHandler, ReferenceGrantHandler, ResourceProcessor, SecretHandler, ServiceHandler,
    TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
};
use crate::core::controller::conf_mgr::sync_runtime::{ShutdownHandle, ShutdownSignal};
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::core::controller::conf_sync::conf_server::ConfigSyncServer;
use edgion_resources::prelude_resources::*;
use edgion_resources::ResourceMeta;

/// Shared state for non-Kubernetes configuration center backends.
///
/// Holds the lifecycle handles and shared runtime that are identical between
/// `FileSystemCenter` and `EtcdCenter`. Each backend embeds this as `base`.
pub struct CenterBase {
    config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>,
    shutdown_handle: Mutex<Option<ShutdownHandle>>,
    controller_handle: Mutex<Option<JoinHandle<()>>>,
    reload_tx: Mutex<Option<mpsc::Sender<()>>>,
    runtime: Arc<ControllerRuntime>,
}

impl CenterBase {
    /// Create a new `CenterBase` with all handles initialised to `None`.
    pub fn new() -> Self {
        Self {
            config_sync_server: RwLock::new(None),
            shutdown_handle: Mutex::new(None),
            controller_handle: Mutex::new(None),
            reload_tx: Mutex::new(None),
            runtime: Arc::new(ControllerRuntime::new()),
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

    fn abort_controller(&self) {
        if let Some(handle) = self.controller_handle.lock().take() {
            handle.abort();
        }
    }

    fn set_reload_tx(&self, tx: Option<mpsc::Sender<()>>) {
        *self.reload_tx.lock() = tx;
    }

    async fn wait_registry_ready(&self, timeout_secs: u64, component: &str) {
        let registry = &self.runtime.processor_registry;
        let timeout = Duration::from_secs(timeout_secs);
        let start = std::time::Instant::now();
        let notify = registry.ready_notify.clone();

        // Phase A: wait for all processors to be registered (Phase 1 + Phase 2).
        while !registry.is_registration_complete() {
            let elapsed = Instant::now().saturating_duration_since(start);
            if elapsed >= timeout {
                tracing::warn!(
                    component = component,
                    timeout_secs = timeout_secs,
                    "Timeout waiting for processor registration to complete, proceeding anyway"
                );
                return;
            }
            let remaining = timeout - elapsed;
            let _ = tokio::time::timeout(remaining.min(Duration::from_secs(1)), notify.notified()).await;
        }

        // Phase B: wait for all registered processors to become ready.
        let remaining = timeout.saturating_sub(Instant::now().saturating_duration_since(start));
        let all_kinds = registry.all_kinds();
        let ready = registry.wait_kinds_ready(&all_kinds, remaining).await;
        if !ready {
            tracing::warn!(
                component = component,
                timeout_secs = timeout_secs,
                not_ready = ?registry.not_ready_kinds(),
                "Timeout waiting for all processors, proceeding anyway"
            );
        }
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

    /// Send a reload request to the running lifecycle loop.
    pub fn request_reload(&self) -> Result<(), String> {
        if let Some(tx) = self.reload_tx.lock().as_ref() {
            tx.try_send(())
                .map_err(|e| format!("Failed to send reload signal: {}", e))
        } else {
            Err("Center not started or not ready for reload".to_string())
        }
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
    ///   the shared runtime, and a oneshot sender for reporting fatal errors.
    pub async fn run_lifecycle(
        &self,
        shutdown_handle: ShutdownHandle,
        component: &'static str,
        make_handle: impl Fn(ShutdownSignal, Arc<ControllerRuntime>, oneshot::Sender<String>) -> JoinHandle<()>,
    ) -> Result<()> {
        self.set_shutdown_handle(shutdown_handle.clone());

        let mut reload_start_time: Option<Instant> = None;

        loop {
            // Record reload completion metrics for non-first iterations.
            if let Some(start_time) = reload_start_time.take() {
                let duration = Instant::now().saturating_duration_since(start_time).as_secs_f64();
                reload_metrics().reload_completed(duration);
                tracing::info!(component = component, duration_secs = duration, "Reload completed");
            }

            // 1. Iteration-specific shutdown handle.
            let iteration_shutdown = ShutdownHandle::new();

            // 2. Reload channel.
            let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);
            self.set_reload_tx(Some(reload_tx));

            // 3. Error channel.
            let (error_tx, error_rx) = oneshot::channel::<String>();

            // 4. Spawn the backend-specific controller.
            let handle = make_handle(iteration_shutdown.signal(), self.runtime.clone(), error_tx);
            self.set_controller_handle(handle);

            // 5. Wait for processor registry to become ready.
            self.wait_registry_ready(30, component).await;

            // 5.5. Post-cache-ready cross-resource revalidations.
            run_post_cache_ready_revalidations(self.runtime.clone()).await;

            // 6. Create ConfigSyncServer and register all WatchObjs.
            let config_sync_server = Arc::new(ConfigSyncServer::new());
            let no_sync_kinds = edgion_common::config::get_no_sync_kinds();
            let no_sync_refs: Vec<&str> = no_sync_kinds.iter().map(|s| s.as_str()).collect();
            config_sync_server.register_all(self.runtime.processor_registry.all_watch_objs(&no_sync_refs));

            // 7. Mark as ready.
            self.set_config_sync_server(Some(config_sync_server), component);

            tracing::info!(
                component = component,
                "ConfigSyncServer is ready, gRPC services can process requests"
            );

            // 8. Wait for shutdown, reload, or controller error.
            let mut global_shutdown = shutdown_handle.signal();
            let mut error_rx = error_rx;

            let action = tokio::select! {
                _ = global_shutdown.wait() => LoopAction::Shutdown,
                _ = reload_rx.recv() => LoopAction::Reload,
                result = &mut error_rx => {
                    match result {
                        Ok(msg) => LoopAction::Error(msg),
                        Err(_) => {
                            global_shutdown.wait().await;
                            LoopAction::Shutdown
                        }
                    }
                }
            };

            // 9. Stop the controller for this iteration.
            iteration_shutdown.shutdown();

            // 10. Cleanup.
            // Clear all caches before dropping CSS — this wakes any active
            // Federation watchers so they detect EventsLost and exit,
            // allowing Center to re-watch on the new CSS after reload.
            if let Some(css) = self.config_sync_server() {
                css.clear_all();
            }
            self.set_config_sync_server(None, component);
            self.abort_controller();
            self.runtime.reset();
            self.set_reload_tx(None);

            // 11. Handle the selected action.
            match action {
                LoopAction::Shutdown => {
                    tracing::info!(component = component, "Normal shutdown, lifecycle completed");
                    return Ok(());
                }
                LoopAction::Reload => {
                    tracing::info!(component = component, "Reload requested, restarting controller");
                    reload_metrics().reload_started();
                    reload_start_time = Some(Instant::now());
                    continue;
                }
                LoopAction::Error(msg) => {
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
        Self::new()
    }
}

/// Action returned by the `run_lifecycle` select loop.
enum LoopAction {
    Shutdown,
    Reload,
    Error(String),
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
    /// - FileSystem: `broadcast::Receiver<FileSystemEvent<String>>`
    type EventReceiver: Send + 'static;

    /// Subscribe to events for a resource kind, returning a channel receiver.
    ///
    /// Must be called before the watcher's `run()` starts sending events.
    async fn subscribe(&self, kind: &'static str) -> Self::EventReceiver;

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
    let capacity = edgion_common::config::get_cache_capacity(kind);
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

/// Spawn all resource controllers (Phase 1 + Phase 2) using the given backend.
///
/// This is the single authoritative resource list shared by both the Etcd and
/// FileSystem controller implementations.  Editing this function is the only
/// place needed when adding or removing a resource type from either backend.
///
/// # Phase ordering
///
/// - **Phase 1** (Foundation): GatewayClass, Gateway, Secret, ConfigMap,
///   ReferenceGrant, Service, and EndpointSlice.
///   The function waits up to 15 s for all Phase-1 processors to become ready
///   before proceeding.
/// - **Phase 2** (Dependent): All route, TLS, plugin, ACME, and config
///   resource types that depend on Phase-1 data being available.
pub async fn spawn_all_controllers<B: LocalControllerBackend>(
    backend: &B,
    shutdown_signal: ShutdownSignal,
    runtime: Arc<ControllerRuntime>,
    component: &'static str,
) -> Result<Vec<JoinHandle<Result<()>>>> {
    use crate::core::controller::conf_mgr::conf_center::common;

    let mut handles = Vec::new();

    // Helper macro: reduces the per-resource boilerplate inside this function.
    macro_rules! do_spawn {
        ($K:ty, $handler:expr) => {
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

    // ==================== Phase 1: Foundation Resources ====================
    tracing::info!(
        component = component,
        "Phase 1: Spawning foundation resource controllers"
    );

    for_each_phase1_core_resource!(do_spawn);

    tracing::info!(component = component, "Registering EndpointSlice controller (Phase 1)");
    do_spawn!(EndpointSlice, EndpointSliceHandler::new());

    let phase1_count = handles.len();
    tracing::info!(
        component = component,
        count = phase1_count,
        "Phase 1 foundation controllers spawned, waiting for init completion"
    );

    // Wait for Phase 1 resources to complete their init phase
    const PHASE1_TIMEOUT: Duration = Duration::from_secs(15);
    let phase1_ready = runtime
        .processor_registry
        .wait_kinds_ready(&common::phase1_kinds(), PHASE1_TIMEOUT)
        .await;

    if phase1_ready {
        tracing::info!(
            component = component,
            "Phase 1 complete: all foundation resources ready"
        );
    } else {
        tracing::warn!(component = component, "Phase 1 timeout: starting Phase 2 anyway");
    }

    // ==================== Phase 2: Dependent Resources ====================
    tracing::info!(
        component = component,
        "Phase 2: Spawning dependent resource controllers"
    );

    for_each_phase2_resource!(do_spawn);

    tracing::info!(
        component = component,
        phase1_count = phase1_count,
        phase2_count = handles.len() - phase1_count,
        total = handles.len(),
        "All resource controllers spawned"
    );

    runtime.processor_registry.mark_registration_complete();

    Ok(handles)
}
