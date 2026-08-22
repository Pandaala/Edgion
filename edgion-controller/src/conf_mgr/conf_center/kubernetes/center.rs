//! KubernetesCenter - Unified configuration center for Kubernetes mode
//!
//! Implements both CenterApi (CRUD) and CenterLifeCycle (lifecycle management),
//! automatically getting ConfCenter via blanket impl.
//!
//! ## Architecture
//!
//! ```text
//! KubernetesCenter
//! ├── writer: KubernetesStorage (CenterApi delegate)
//! ├── config: KubernetesConfig
//! ├── config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>
//! ├── shutdown_handle: Mutex<Option<ShutdownHandle>>
//! └── client: kube::Client
//! ```

use super::config::{HaMode, KubernetesConfig};
use super::controller::{ControllerExitReason, KubernetesController};
use super::leader_election::{LeaderElection, LeaderElectionConfig as InternalLeaderElectionConfig, LeaderHandle};
use super::storage::KubernetesStorage;
use super::version_detection::{preflight_check_gateway_api_crds, validate_no_watch_kinds};
use crate::conf_mgr::conf_center::traits::{
    impl_center_api_delegate, CenterApi, CenterLifeCycle, ConfWriterError, ListOptions, ListResult, ResourceKey,
};
use crate::conf_mgr::sync_runtime::metrics::reload_metrics;
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use crate::conf_mgr::ControllerRuntime;
use crate::conf_sync::conf_server::ConfigSyncServer;
use anyhow::Result;
use async_trait::async_trait;
use kube::Client;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Give up (trigger graceful shutdown) after this many consecutive failures.
const MAX_CONSECUTIVE_FAILURES: u32 = 10;
/// A flow iteration that survived this long resets the failure counter.
const STABLE_RUN_DURATION: Duration = Duration::from_secs(300);

/// Shared failure-counting/backoff/give-up policy for both flows
/// (leader-only `run_main_flow` and all-serve `run_all_serve_lifecycle`).
struct FailureBudget {
    consecutive_failures: u32,
}

/// What to do after a failure exit.
enum FailureAction {
    /// Sleep this long, then retry.
    Retry(Duration),
    /// Budget exhausted — trigger graceful shutdown and stop retrying.
    GiveUp,
}

impl FailureBudget {
    fn new() -> Self {
        Self {
            consecutive_failures: 0,
        }
    }

    /// Benign exit (manual reload, 410 relink): failures were transient noise.
    fn reset(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Record a failure exit. `ran_stably` = the iteration ran at least
    /// STABLE_RUN_DURATION before failing (reset-before-count, matching the
    /// pre-existing run_main_flow semantics).
    fn on_failure(&mut self, ran_stably: bool) -> FailureAction {
        if ran_stably {
            self.consecutive_failures = 0;
        }
        self.consecutive_failures += 1;
        if self.consecutive_failures > MAX_CONSECUTIVE_FAILURES {
            FailureAction::GiveUp
        } else {
            FailureAction::Retry(KubernetesCenter::backoff(self.consecutive_failures))
        }
    }
}

/// Exit reason from main flow
enum MainFlowExit {
    /// Normal shutdown requested (Ctrl+C, SIGTERM)
    Shutdown,
    /// Lost leadership, need to wait for re-election
    LostLeadership,
}

/// Lifecycle event for event-driven architecture
#[derive(Debug)]
enum LifecycleEvent {
    /// Caches are ready (PROCESSOR_REGISTRY is ready)
    CachesReady,
    /// Caches ready timeout - not all caches ready within timeout
    CachesTimeout,
    /// Lost leadership
    LeadershipLost,
    /// Acquired leadership (all-serve mode only)
    LeadershipAcquired,
    /// Controller exited
    ControllerExit(ControllerExitReason),
    /// Manual reload requested via Admin API
    ReloadRequested,
}

/// Exit reason from all-serve serving flow.
enum ServingFlowExit {
    /// Normal shutdown requested
    Shutdown,
    /// Benign restart (manual reload or 410 GONE relink) — resets the failure budget
    Benign,
    /// Real failure (caches timeout / watcher panic / deadline / channel closed) — counts toward give-up
    Failure,
}

/// One turn of the supervised event loop (see `next_loop_turn`).
#[derive(Debug)]
enum LoopTurn {
    /// A lifecycle event arrived — dispatch to the flow's match arms.
    Event(LifecycleEvent),
    /// `event_rx` closed (all senders dropped) — treated as a failure.
    ChannelClosed,
    /// A watcher task completed normally (they send their event, then return).
    /// The caller debug-logs and continues the loop.
    WatcherFinished(&'static str),
    /// A watcher task panicked (in-loop JoinError can only be a panic: our own
    /// aborts happen only in post-break cleanup).
    WatcherPanicked(&'static str),
    /// The pre-ready deadline elapsed before CachesReady.
    DeadlineExpired,
}

/// Supervised set of watcher tasks for one event-loop iteration, plus the
/// iteration's leadership-epoch cancellation token.
struct WatcherSet {
    set: JoinSet<&'static str>,
    names: HashMap<tokio::task::Id, &'static str>,
    cancel: CancellationToken,
}

impl WatcherSet {
    fn new(cancel: CancellationToken) -> Self {
        Self {
            set: JoinSet::new(),
            names: HashMap::new(),
            cancel,
        }
    }

    /// Clone of the iteration's cancellation token (for the caches task and,
    /// in leader-only mode, the leadership watcher).
    fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Spawn a named watcher. The task's return value is its name, so normal
    /// completion self-identifies; the id→name map covers the panic path.
    fn spawn<F>(&mut self, name: &'static str, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let abort = self.set.spawn(async move {
            fut.await;
            name
        });
        self.names.insert(abort.id(), name);
    }

    /// Cooperative cancel first, then abort all tasks and wait for them.
    async fn abort_and_wait(mut self) {
        self.cancel.cancel();
        self.set.abort_all();
        while self.set.join_next().await.is_some() {}
    }
}

/// One `select!` turn of the supervised event loop. Pure with respect to
/// `KubernetesCenter` state so it is unit-testable with plain tokio.
///
/// `biased` drains queued events before join results: every watcher sends its
/// event before returning, so normal completion never hides an event — even if
/// a `WatcherFinished` turn wins a race, the event is delivered on the next turn.
async fn next_loop_turn(
    event_rx: &mut mpsc::Receiver<LifecycleEvent>,
    watchers: &mut WatcherSet,
    deadline: Pin<&mut tokio::time::Sleep>,
    deadline_armed: bool,
) -> LoopTurn {
    tokio::select! {
        biased;
        ev = event_rx.recv() => match ev {
            Some(event) => LoopTurn::Event(event),
            None => LoopTurn::ChannelClosed,
        },
        Some(res) = watchers.set.join_next_with_id(), if !watchers.set.is_empty() => match res {
            Ok((_id, name)) => LoopTurn::WatcherFinished(name),
            Err(join_err) => {
                let name = watchers.names.get(&join_err.id()).copied().unwrap_or("unknown");
                LoopTurn::WatcherPanicked(name)
            }
        },
        _ = deadline, if deadline_armed => LoopTurn::DeadlineExpired,
    }
}

/// KubernetesCenter - Configuration center for Kubernetes mode
///
/// This struct implements both `CenterApi` and `CenterLifeCycle`,
/// automatically getting `ConfCenter` implementation via blanket impl.
pub struct KubernetesCenter {
    /// Configuration
    config: KubernetesConfig,
    /// Writer for CRUD operations (delegate)
    writer: KubernetesStorage,
    /// ConfigSyncServer instance for gRPC list/watch
    /// None: Not ready (startup, restart, leadership loss)
    /// Some: Ready to serve requests
    config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>,
    /// Shutdown handle for stopping sync tasks
    shutdown_handle: Mutex<Option<ShutdownHandle>>,
    /// Reload signal sender (for triggering reload via Admin API)
    reload_tx: Mutex<Option<mpsc::Sender<()>>>,
    /// Aggregated controller runtime (stores, registries)
    runtime: Arc<ControllerRuntime>,
    /// Live leadership flag published to process-wide singleton components via
    /// `CenterLifeCycle::leader_signal` (today: the federation client
    /// supervisor). Created here in `new()` — NOT in `start()` — so there is no
    /// window in which a subscriber could ask before the signal exists. The same
    /// sender is handed to `LeaderElection` in `start()`, which is the only
    /// writer.
    leader_flag: Arc<watch::Sender<bool>>,
}

impl KubernetesCenter {
    /// Create a new KubernetesCenter
    pub async fn new(config: KubernetesConfig) -> Result<Self> {
        config.validate()?;

        // Kubernetes ingest always deserializes JSON via `kube` (`serde_json`
        // under the hood), so the supported resource-spec unknown-field walk's `Value`
        // round-trip is lossless here and can never turn a loadable resource
        // into a load failure — see `schema_check::strict_ingest_enabled`
        // for the full rationale. Set exactly once,
        // before any resource is watched/deserialized off this center.
        edgion_resources::schema_check::set_strict_ingest(true);

        tracing::info!(
            component = "kubernetes_center",
            mode = "kubernetes",
            gateway_class = %config.gateway_class(),
            "Creating KubernetesCenter"
        );

        let writer = KubernetesStorage::new(config.watch_namespaces.clone()).await?;

        Ok(Self {
            config,
            writer,
            config_sync_server: RwLock::new(None),
            shutdown_handle: Mutex::new(None),
            reload_tx: Mutex::new(None),
            runtime: Arc::new(ControllerRuntime::new()),
            leader_flag: Arc::new(watch::channel(false).0),
        })
    }

    /// Get the configuration
    pub fn config(&self) -> &KubernetesConfig {
        &self.config
    }

    // ==================== Helper Methods ====================

    /// Set the ConfigSyncServer (Some = ready, None = not ready)
    fn set_config_sync_server(&self, server: Option<Arc<ConfigSyncServer>>) {
        let mut sync_server = self.config_sync_server.write();
        let was_ready = sync_server.is_some();
        let is_ready = server.is_some();
        *sync_server = server;

        if was_ready != is_ready {
            tracing::info!(
                component = "kubernetes_center",
                event = "config_sync_server_state_changed",
                was_ready = was_ready,
                is_ready = is_ready,
                "ConfigSyncServer state changed"
            );
        }
    }

    /// Store shutdown handle for lifecycle management
    fn set_shutdown_handle(&self, handle: ShutdownHandle) {
        let mut shutdown_handle = self.shutdown_handle.lock();
        *shutdown_handle = Some(handle);
    }

    /// Set reload signal sender
    fn set_reload_tx(&self, tx: Option<mpsc::Sender<()>>) {
        *self.reload_tx.lock() = tx;
    }

    /// Create internal LeaderElectionConfig from KubernetesConfig
    fn create_leader_election_config(&self) -> Result<InternalLeaderElectionConfig> {
        let le_config = self.config.leader_election();

        // Create internal leader election config from serialized config
        let config = InternalLeaderElectionConfig::new(&le_config.lease_name, &le_config.lease_namespace)?
            .with_lease_duration_secs(le_config.lease_duration_secs)
            .with_renew_deadline_secs(le_config.renew_deadline_secs)
            .with_retry_period_secs(le_config.retry_period_secs);

        tracing::info!(
            component = "kubernetes_center",
            mode = "kubernetes",
            lease_name = %le_config.lease_name,
            lease_namespace = %le_config.lease_namespace,
            lease_duration_secs = le_config.lease_duration_secs,
            "Leader election configuration loaded"
        );

        Ok(config)
    }

    /// Create the Kubernetes resource controller (no ConfigServer dependency).
    fn create_k8s_controller(&self, client: &Client) -> Result<KubernetesController> {
        let config = self.config();

        tracing::info!(
            component = "kubernetes_center",
            mode = "kubernetes",
            gateway_class = %config.gateway_class(),
            controller_name = %config.controller_name(),
            namespaces = ?config.watch_namespaces(),
            metadata_filter_enabled = true,
            blocked_annotations_count = config.metadata_filter().blocked_annotations.len(),
            remove_managed_fields = config.metadata_filter().remove_managed_fields,
            "Creating Kubernetes controller"
        );

        let no_watch_kinds = config.no_watch_kinds.iter().cloned().collect();
        KubernetesController::with_metadata_filter(
            client.clone(),
            config.gateway_class.clone(),
            config.controller_name.clone(),
            config.gateway_address.clone(),
            config.watch_namespaces.clone(),
            config.label_selector.clone(),
            config.metadata_filter.clone(),
            no_watch_kinds,
        )
    }

    /// Main flow - runs only when this instance is the leader
    ///
    /// Event-driven architecture:
    /// 1. Start all event watcher tasks (controller, caches, leadership)
    /// 2. Process events in a simple match loop
    /// 3. Handle retry logic for errors and 410 GONE
    async fn run_main_flow(
        &self,
        client: &Client,
        leader_handle: &LeaderHandle,
        shutdown_handle: &ShutdownHandle,
    ) -> MainFlowExit {
        let mut budget = FailureBudget::new();
        let mut reload_start_time: Option<std::time::Instant> = None;
        let deadline_budget = Duration::from_secs(self.config.cache_ready_deadline_secs);

        loop {
            // Record reload completion time if this is a reload iteration
            if let Some(start_time) = reload_start_time.take() {
                let duration = Instant::now().saturating_duration_since(start_time).as_secs_f64();
                reload_metrics().reload_completed(duration);
                tracing::info!(
                    component = "kubernetes_center",
                    duration_secs = duration,
                    "Reload completed"
                );
            }

            // Check if still leader before starting iteration
            if !leader_handle.is_leader() {
                self.set_config_sync_server(None);
                return MainFlowExit::LostLeadership;
            }

            tracing::info!(
                component = "kubernetes_center",
                mode = "kubernetes",
                "Starting event watchers"
            );

            // Create event channel
            let (event_tx, mut event_rx) = mpsc::channel::<LifecycleEvent>(32);

            // The deadline budget also bounds start_event_watchers itself:
            // preflight_check_gateway_api_crds is awaited before any task
            // spawns, and without this wrap a hung preflight would be bounded
            // only by the kube client's own request timeout.
            let started = tokio::time::timeout(
                deadline_budget,
                self.start_event_watchers(client, shutdown_handle, leader_handle, event_tx),
            )
            .await;
            let (mut watchers, config_sync_server) = match started {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    tracing::error!(
                        component = "kubernetes_center",
                        mode = "kubernetes",
                        error = %e,
                        "Failed to start event watchers"
                    );
                    match budget.on_failure(false) {
                        FailureAction::Retry(delay) => {
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        FailureAction::GiveUp => {
                            tracing::error!(
                                component = "kubernetes_center",
                                mode = "kubernetes",
                                "Max consecutive failures exceeded, triggering graceful shutdown"
                            );
                            shutdown_handle.shutdown();
                            return MainFlowExit::Shutdown;
                        }
                    }
                }
                Err(_elapsed) => {
                    tracing::error!(
                        component = "kubernetes_center",
                        mode = "kubernetes",
                        deadline_secs = deadline_budget.as_secs(),
                        "start_event_watchers exceeded cache_ready_deadline_secs (hung preflight?)"
                    );
                    match budget.on_failure(false) {
                        FailureAction::Retry(delay) => {
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        FailureAction::GiveUp => {
                            shutdown_handle.shutdown();
                            return MainFlowExit::Shutdown;
                        }
                    }
                }
            };

            let iteration_start = Instant::now();
            let mut caches_ready = false;
            let mut deadline = Box::pin(tokio::time::sleep(deadline_budget));

            // Supervised event loop: events / watcher joins / pre-ready deadline.
            let exit_reason = loop {
                let turn = next_loop_turn(&mut event_rx, &mut watchers, deadline.as_mut(), !caches_ready).await;
                match turn {
                    LoopTurn::Event(event) => match event {
                        LifecycleEvent::CachesReady => {
                            caches_ready = true;
                            self.set_config_sync_server(Some(config_sync_server.clone()));
                            tracing::info!(
                                component = "kubernetes_center",
                                mode = "kubernetes",
                                "ConfigSyncServer is ready, gRPC services can process requests"
                            );
                        }
                        LifecycleEvent::CachesTimeout => {
                            tracing::error!(
                                component = "kubernetes_center",
                                mode = "kubernetes",
                                "Caches timeout, treating as controller failure"
                            );
                            break ControllerExitReason::AllControllersStopped;
                        }
                        LifecycleEvent::LeadershipLost => {
                            tracing::warn!(component = "kubernetes_center", mode = "kubernetes", "Lost leadership");
                            break ControllerExitReason::LostLeadership;
                        }
                        LifecycleEvent::LeadershipAcquired => {
                            // Not expected in leader-only mode, ignore
                        }
                        LifecycleEvent::ControllerExit(reason) => {
                            tracing::info!(
                                component = "kubernetes_center",
                                mode = "kubernetes",
                                reason = ?reason,
                                "Controller exited"
                            );
                            break reason;
                        }
                        LifecycleEvent::ReloadRequested => {
                            tracing::info!(
                                component = "kubernetes_center",
                                mode = "kubernetes",
                                "Reload requested via Admin API, restarting controllers"
                            );
                            break ControllerExitReason::RelinkRequested(
                                super::resource_controller::RelinkReason::ReloadRequested,
                            );
                        }
                    },
                    LoopTurn::ChannelClosed => {
                        tracing::error!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            "Event channel closed unexpectedly"
                        );
                        break ControllerExitReason::AllControllersStopped;
                    }
                    LoopTurn::WatcherFinished(name) => {
                        tracing::debug!(component = "kubernetes_center", task = name, "Watcher task completed");
                        continue;
                    }
                    LoopTurn::WatcherPanicked(name) => {
                        tracing::error!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            task = name,
                            "Watcher task panicked, treating as controller failure"
                        );
                        break ControllerExitReason::AllControllersStopped;
                    }
                    LoopTurn::DeadlineExpired => {
                        tracing::warn!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            deadline_secs = deadline_budget.as_secs(),
                            "Pre-ready deadline exceeded before CachesReady; raise \
                             cache_ready_deadline_secs if this cluster legitimately needs longer"
                        );
                        break ControllerExitReason::AllControllersStopped;
                    }
                }
            };

            // Cleanup: cancel the epoch token, abort and wait for all watcher tasks
            watchers.abort_and_wait().await;

            // Clear config_sync_server if it was set
            if caches_ready {
                self.set_config_sync_server(None);
            }

            // Stop leader-only jobs and reset ControllerRuntime for retry
            Self::stop_leader_jobs();
            tracing::info!(component = "kubernetes_center", "ControllerRuntime reset");
            self.runtime.reset();

            // Handle exit reason
            match exit_reason {
                ControllerExitReason::Shutdown => {
                    return MainFlowExit::Shutdown;
                }
                ControllerExitReason::LostLeadership => {
                    return MainFlowExit::LostLeadership;
                }
                ControllerExitReason::RelinkRequested(reason) => {
                    // Check if this is a manual reload request
                    let is_reload = matches!(reason, super::resource_controller::RelinkReason::ReloadRequested);
                    if is_reload {
                        tracing::info!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            "Manual reload requested, restarting with new server_id"
                        );
                        reload_metrics().reload_started();
                        reload_start_time = Some(std::time::Instant::now());
                    } else {
                        tracing::info!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            reason = ?reason,
                            "Relink requested (410 GONE), restarting immediately"
                        );
                    }
                    budget.reset();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                ControllerExitReason::AllControllersStopped => {
                    // Real error - count as failure and apply backoff
                    tracing::warn!(
                        component = "kubernetes_center",
                        mode = "kubernetes",
                        "All controllers stopped, will restart with backoff"
                    );

                    let ran_stably = Instant::now().saturating_duration_since(iteration_start) >= STABLE_RUN_DURATION;
                    match budget.on_failure(ran_stably) {
                        FailureAction::Retry(delay) => {
                            tracing::info!(
                                component = "kubernetes_center",
                                mode = "kubernetes",
                                backoff_secs = delay.as_secs(),
                                "Waiting before restart"
                            );
                            tokio::time::sleep(delay).await;
                        }
                        FailureAction::GiveUp => {
                            tracing::error!(
                                component = "kubernetes_center",
                                mode = "kubernetes",
                                "Max consecutive failures exceeded, triggering graceful shutdown"
                            );
                            shutdown_handle.shutdown();
                            return MainFlowExit::Shutdown;
                        }
                    }
                }
            }
        }
    }

    /// Start all event watcher tasks
    ///
    /// This method creates ConfigSyncServer, Controller, and spawns all event watcher tasks:
    /// 1. Controller task - runs controller.run() and sends ControllerExit event
    /// 2. Caches watcher task - monitors PROCESSOR_REGISTRY readiness and sends CachesReady event
    /// 3. Leadership watcher task - monitors leadership and sends LeadershipLost event
    ///
    /// Returns:
    /// - `WatcherSet`: Supervised set of watcher tasks (with the iteration's
    ///   leadership-epoch cancellation token) to drive/abort the event loop
    /// - `Arc<ConfigSyncServer>`: The config sync server for setting service availability
    async fn start_event_watchers(
        &self,
        client: &Client,
        shutdown_handle: &ShutdownHandle,
        leader_handle: &LeaderHandle,
        event_tx: mpsc::Sender<LifecycleEvent>,
    ) -> Result<(WatcherSet, Arc<ConfigSyncServer>)> {
        // 1. Validate and preflight check Gateway API CRDs
        validate_no_watch_kinds(&self.config.no_watch_kinds);
        let no_watch_kinds: std::collections::HashSet<String> = self.config.no_watch_kinds.iter().cloned().collect();
        preflight_check_gateway_api_crds(client, &no_watch_kinds).await?;

        // 2. Create ConfigSyncServer (will be populated when caches are ready)
        let config_sync_server = Arc::new(ConfigSyncServer::new());

        // 3. Create Controller and leader handle
        let controller = self
            .create_k8s_controller(client)?
            .with_leader_handle(leader_handle.clone());
        let expected_processor_kinds = controller.all_processor_kinds();

        let mut watchers = WatcherSet::new(CancellationToken::new());

        // 4. Controller task
        let shutdown_signal = shutdown_handle.signal();
        let tx = event_tx.clone();
        let runtime_for_controller = self.runtime.clone();
        watchers.spawn("controller", async move {
            let reason = controller
                .run(shutdown_signal, runtime_for_controller)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(
                        component = "kubernetes_center",
                        mode = "kubernetes",
                        error = %e,
                        "Controller run error"
                    );
                    ControllerExitReason::AllControllersStopped
                });
            let _ = tx.send(LifecycleEvent::ControllerExit(reason)).await;
        });

        // 5. Caches-ready watcher (starts leader jobs on ready — leader-only mode)
        let no_sync_kinds = crate::config::get_no_sync_kinds();
        watchers.spawn(
            "caches",
            Self::run_caches_ready(
                config_sync_server.clone(),
                self.runtime.clone(),
                expected_processor_kinds,
                no_sync_kinds,
                event_tx.clone(),
                Some(client.clone()), // start ACME when ready
                self.config.watch_namespaces.clone(),
                watchers.cancel_token(),
            ),
        );

        // 6. Leadership-loss watcher. Leader-only: cancels the epoch token FIRST,
        // then sends LeadershipLost — the caches task is stopped even while the
        // event loop is busy. (All-serve wiring is DIFFERENT — cleanup-only
        // cancel; see start_event_watchers_all_serve. Do not unify them.)
        let lh = leader_handle.clone();
        let tx = event_tx.clone();
        let epoch_cancel = watchers.cancel_token();
        watchers.spawn("leadership", async move {
            while lh.is_leader() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            epoch_cancel.cancel();
            let _ = tx.send(LifecycleEvent::LeadershipLost).await;
        });

        // 7. Reload-signal watcher
        let reload_fut = self.reload_watcher_future(event_tx);
        watchers.spawn("reload", reload_fut);

        Ok((watchers, config_sync_server))
    }

    /// Leader-only lifecycle: identical to pre-HA behavior.
    /// Wait for leadership -> run_main_flow -> handle exit.
    async fn run_leader_only_lifecycle(
        &self,
        client: &Client,
        leader_handle: &LeaderHandle,
        shutdown_handle: &ShutdownHandle,
    ) -> Result<()> {
        loop {
            tracing::info!(
                component = "kubernetes_center",
                ha_mode = "leader-only",
                "Waiting for leadership..."
            );

            if !leader_handle
                .wait_until_leader_with_shutdown(shutdown_handle.signal())
                .await
            {
                tracing::info!(
                    component = "kubernetes_center",
                    ha_mode = "leader-only",
                    "Shutdown requested before acquiring leadership"
                );
                return Ok(());
            }

            tracing::info!(
                component = "kubernetes_center",
                ha_mode = "leader-only",
                "Acquired leadership, entering main flow"
            );

            let exit_reason = self.run_main_flow(client, leader_handle, shutdown_handle).await;

            match exit_reason {
                MainFlowExit::Shutdown => {
                    tracing::info!(
                        component = "kubernetes_center",
                        ha_mode = "leader-only",
                        "Normal shutdown"
                    );
                    tracing::info!(component = "kubernetes_center", "ControllerRuntime reset");
                    self.runtime.reset();
                    return Ok(());
                }
                MainFlowExit::LostLeadership => {
                    tracing::warn!(
                        component = "kubernetes_center",
                        ha_mode = "leader-only",
                        "Lost leadership, will wait for re-election"
                    );
                    tracing::info!(component = "kubernetes_center", "ControllerRuntime reset");
                    self.runtime.reset();
                    continue;
                }
            }
        }
    }

    /// Calculate exponential backoff duration
    fn backoff(failures: u32) -> Duration {
        Duration::from_secs(1 << failures.min(6))
    }

    /// Caches-ready watcher body (spawned into the iteration's `WatcherSet`).
    ///
    /// Polls until all expected processor kinds are registered and synced, then
    /// runs the tail: WatchObj registration → cross-namespace revalidations →
    /// (leader-only) leader-job start → `CachesReady`. The `cancel` token is the
    /// leadership-epoch guard: a demoted replica's tail is stopped instead of
    /// racing, and a cancelled task sends NEITHER CachesReady NOR starts jobs —
    /// suppressing both together is what prevents the "reports ready with all
    /// leader jobs dead" state (see the wedge-recovery design spec).
    #[allow(clippy::too_many_arguments)]
    async fn run_caches_ready(
        css: Arc<ConfigSyncServer>,
        runtime: Arc<ControllerRuntime>,
        expected_processor_kinds: Vec<&'static str>,
        no_sync_kinds: Vec<String>,
        tx: mpsc::Sender<LifecycleEvent>,
        acme_client: Option<Client>,
        watch_namespaces: Vec<String>,
        cancel: CancellationToken,
    ) {
        const CACHE_READY_TIMEOUT_SECS: u64 = 30;
        let timeout = Duration::from_secs(CACHE_READY_TIMEOUT_SECS);
        let start = Instant::now();
        let no_sync_refs: Vec<&str> = no_sync_kinds.iter().map(|s| s.as_str()).collect();
        let registry = &runtime.processor_registry;
        // Kinds that must be both registered and ready (excludes no_sync_kinds)
        let expected_sync_kinds: Vec<&'static str> = expected_processor_kinds
            .iter()
            .copied()
            .filter(|k| !no_sync_kinds.iter().any(|ns| ns == *k))
            .collect();

        let pending_sync_kinds = || -> Vec<&'static str> {
            let missing_registration: Vec<&'static str> = expected_processor_kinds
                .iter()
                .copied()
                .filter(|k| registry.get(k).is_none())
                .collect();
            if !missing_registration.is_empty() {
                return missing_registration;
            }

            expected_sync_kinds
                .iter()
                .copied()
                .filter(|k| registry.get(k).map(|p| !p.is_ready()).unwrap_or(true))
                .collect()
        };

        let mut pending = pending_sync_kinds();
        while !pending.is_empty() && Instant::now().saturating_duration_since(start) < timeout {
            tokio::time::sleep(Duration::from_millis(200)).await;
            pending = pending_sync_kinds();
        }

        if pending.is_empty() {
            // Cancellable tail: semantically equivalent to the existing
            // abort_and_wait (drop at an await point), just earlier.
            let tail = async {
                css.register_all(registry.all_watch_objs(&no_sync_refs));
                crate::conf_mgr::sync_runtime::resource_processor::run_post_cache_ready_revalidations(runtime.clone())
                    .await;
            };
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tail => {}
            }

            // Hard checkpoint: after demotion, neither start jobs nor report
            // ready. From here to the send there is no await before the job
            // start, so a cancel can no longer interleave between the check
            // and the (synchronous) start_leader_jobs.
            if cancel.is_cancelled() {
                return;
            }
            if let Some(client) = acme_client {
                Self::start_leader_jobs(&client, &watch_namespaces);
            }
            let _ = tx.send(LifecycleEvent::CachesReady).await;
        } else {
            tracing::warn!(
                component = "kubernetes_center",
                timeout_secs = CACHE_READY_TIMEOUT_SECS,
                not_ready = ?pending,
                "Timeout waiting for processors"
            );
            let _ = tx.send(LifecycleEvent::CachesTimeout).await;
        }
    }

    /// Build the reload-signal watcher future (spawned into the WatcherSet).
    fn reload_watcher_future(&self, tx: mpsc::Sender<LifecycleEvent>) -> impl std::future::Future<Output = ()> {
        let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);
        self.set_reload_tx(Some(reload_tx));
        async move {
            if reload_rx.recv().await.is_some() {
                let _ = tx.send(LifecycleEvent::ReloadRequested).await;
            }
        }
    }

    // ==================== All-Serve Mode Methods ====================

    /// All-serve lifecycle: all replicas run watchers + gRPC.
    /// Leader-only services (status writes, ACME) start/stop dynamically.
    async fn run_all_serve_lifecycle(
        &self,
        client: &Client,
        leader_handle: &LeaderHandle,
        shutdown_handle: &ShutdownHandle,
    ) -> Result<()> {
        let mut budget = FailureBudget::new();
        loop {
            let iteration_start = Instant::now();
            let exit = self.run_serving_flow(client, leader_handle, shutdown_handle).await;
            match exit {
                ServingFlowExit::Shutdown => return Ok(()),
                ServingFlowExit::Benign => {
                    budget.reset();
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                ServingFlowExit::Failure => {
                    let ran_stably = Instant::now().saturating_duration_since(iteration_start) >= STABLE_RUN_DURATION;
                    match budget.on_failure(ran_stably) {
                        FailureAction::Retry(delay) => {
                            tracing::info!(
                                component = "kubernetes_center",
                                ha_mode = "all-serve",
                                backoff_secs = delay.as_secs(),
                                "Waiting before restart"
                            );
                            tokio::time::sleep(delay).await;
                        }
                        FailureAction::GiveUp => {
                            tracing::error!(
                                component = "kubernetes_center",
                                ha_mode = "all-serve",
                                "Max consecutive failures exceeded, triggering graceful shutdown"
                            );
                            shutdown_handle.shutdown();
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    /// Core event loop for all-serve mode.
    /// All replicas start watchers immediately (no wait_until_leader).
    /// Leader services start/stop dynamically based on leadership state.
    async fn run_serving_flow(
        &self,
        client: &Client,
        leader_handle: &LeaderHandle,
        shutdown_handle: &ShutdownHandle,
    ) -> ServingFlowExit {
        let deadline_budget = Duration::from_secs(self.config.cache_ready_deadline_secs);
        let (event_tx, mut event_rx) = mpsc::channel::<LifecycleEvent>(32);

        // The deadline budget also bounds start_event_watchers itself:
        // preflight_check_gateway_api_crds is awaited before any task spawns,
        // and without this wrap a hung preflight would be bounded only by the
        // kube client's own request timeout.
        let started = tokio::time::timeout(
            deadline_budget,
            self.start_event_watchers_all_serve(client, shutdown_handle, leader_handle, event_tx),
        )
        .await;
        let (mut watchers, config_sync_server) = match started {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                tracing::error!(
                    component = "kubernetes_center",
                    error = %e,
                    "Failed to start event watchers (all-serve)"
                );
                return ServingFlowExit::Failure;
            }
            Err(_elapsed) => {
                tracing::error!(
                    component = "kubernetes_center",
                    deadline_secs = deadline_budget.as_secs(),
                    "start_event_watchers exceeded cache_ready_deadline_secs (all-serve)"
                );
                return ServingFlowExit::Failure;
            }
        };

        let mut caches_ready = false;
        let mut leader_services_running = false;
        let mut deadline = Box::pin(tokio::time::sleep(deadline_budget));

        // Supervised event loop: events / watcher joins / pre-ready deadline.
        let exit_reason = loop {
            let turn = next_loop_turn(&mut event_rx, &mut watchers, deadline.as_mut(), !caches_ready).await;
            match turn {
                LoopTurn::Event(event) => match event {
                    LifecycleEvent::CachesReady => {
                        caches_ready = true;
                        self.set_config_sync_server(Some(config_sync_server.clone()));
                        tracing::info!(
                            component = "kubernetes_center",
                            ha_mode = "all-serve",
                            "ConfigSyncServer is ready"
                        );

                        if leader_handle.is_leader() && !leader_services_running {
                            self.start_leader_services(client).await;
                            leader_services_running = true;
                        }
                    }
                    LifecycleEvent::LeadershipAcquired => {
                        tracing::info!(
                            component = "kubernetes_center",
                            ha_mode = "all-serve",
                            "Leadership acquired"
                        );
                        if caches_ready && !leader_services_running {
                            self.start_leader_services(client).await;
                            leader_services_running = true;
                        }
                    }
                    LifecycleEvent::LeadershipLost => {
                        tracing::info!(
                            component = "kubernetes_center",
                            ha_mode = "all-serve",
                            "Leadership lost, stopping leader services but continuing gRPC"
                        );
                        if leader_services_running {
                            self.stop_leader_services().await;
                            leader_services_running = false;
                        }
                    }
                    LifecycleEvent::CachesTimeout => {
                        break ControllerExitReason::AllControllersStopped;
                    }
                    LifecycleEvent::ControllerExit(reason) => {
                        break reason;
                    }
                    LifecycleEvent::ReloadRequested => {
                        break ControllerExitReason::RelinkRequested(
                            super::resource_controller::RelinkReason::ReloadRequested,
                        );
                    }
                },
                LoopTurn::ChannelClosed => {
                    tracing::error!(
                        component = "kubernetes_center",
                        ha_mode = "all-serve",
                        "Event channel closed unexpectedly"
                    );
                    break ControllerExitReason::AllControllersStopped;
                }
                LoopTurn::WatcherFinished(name) => {
                    tracing::debug!(component = "kubernetes_center", task = name, "Watcher task completed");
                    continue;
                }
                LoopTurn::WatcherPanicked(name) => {
                    tracing::error!(
                        component = "kubernetes_center",
                        ha_mode = "all-serve",
                        task = name,
                        "Watcher task panicked, treating as controller failure"
                    );
                    break ControllerExitReason::AllControllersStopped;
                }
                LoopTurn::DeadlineExpired => {
                    tracing::warn!(
                        component = "kubernetes_center",
                        ha_mode = "all-serve",
                        deadline_secs = deadline_budget.as_secs(),
                        "Pre-ready deadline exceeded before CachesReady; raise \
                         cache_ready_deadline_secs if this cluster legitimately needs longer"
                    );
                    break ControllerExitReason::AllControllersStopped;
                }
            }
        };

        // Cleanup
        watchers.abort_and_wait().await;
        if caches_ready {
            self.set_config_sync_server(None);
        }
        if leader_services_running {
            self.stop_leader_services().await;
        }
        tracing::info!(component = "kubernetes_center", "ControllerRuntime reset");
        self.runtime.reset();

        match exit_reason {
            ControllerExitReason::Shutdown => ServingFlowExit::Shutdown,
            ControllerExitReason::RelinkRequested(_) => ServingFlowExit::Benign,
            // LostLeadership bubbling up via ControllerExit is a leadership
            // transition, not a fault — the loop normally handles it inline.
            ControllerExitReason::LostLeadership => ServingFlowExit::Benign,
            ControllerExitReason::AllControllersStopped => ServingFlowExit::Failure,
        }
    }

    /// Start event watchers for all-serve mode.
    ///
    /// Differs from leader-only in two ways:
    /// - ACME is **not** started here; it starts/stops via `start/stop_leader_services`.
    /// - Leadership watcher fires **bidirectional** events (Acquired + Lost), not loss-only.
    async fn start_event_watchers_all_serve(
        &self,
        client: &Client,
        shutdown_handle: &ShutdownHandle,
        leader_handle: &LeaderHandle,
        event_tx: mpsc::Sender<LifecycleEvent>,
    ) -> Result<(WatcherSet, Arc<ConfigSyncServer>)> {
        // 1. Validate and preflight check Gateway API CRDs
        validate_no_watch_kinds(&self.config.no_watch_kinds);
        let no_watch_kinds: std::collections::HashSet<String> = self.config.no_watch_kinds.iter().cloned().collect();
        preflight_check_gateway_api_crds(client, &no_watch_kinds).await?;

        // 2. Create ConfigSyncServer
        let config_sync_server = Arc::new(ConfigSyncServer::new());

        // 3. Create controller with leader_handle for status write gating
        let controller = self
            .create_k8s_controller(client)?
            .with_leader_handle(leader_handle.clone());
        let expected_processor_kinds = controller.all_processor_kinds();

        let mut watchers = WatcherSet::new(CancellationToken::new());

        // 4. Controller task
        let shutdown_signal = shutdown_handle.signal();
        let tx = event_tx.clone();
        let runtime_for_controller = self.runtime.clone();
        watchers.spawn("controller", async move {
            let reason = controller
                .run(shutdown_signal, runtime_for_controller)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(
                        component = "kubernetes_center",
                        ha_mode = "all-serve",
                        error = %e,
                        "Controller run error"
                    );
                    ControllerExitReason::AllControllersStopped
                });
            let _ = tx.send(LifecycleEvent::ControllerExit(reason)).await;
        });

        // 5. Caches-ready watcher (no ACME — started via start_leader_services).
        // The token still guards the tail + CachesReady send, but is cancelled
        // only by cleanup (abort_and_wait), never by the leadership watcher.
        let no_sync_kinds = crate::config::get_no_sync_kinds();
        watchers.spawn(
            "caches",
            Self::run_caches_ready(
                config_sync_server.clone(),
                self.runtime.clone(),
                expected_processor_kinds,
                no_sync_kinds,
                event_tx.clone(),
                None, // ACME starts via start_leader_services
                self.config.watch_namespaces.clone(),
                watchers.cancel_token(),
            ),
        );

        // 6. Leadership watcher — bidirectional (Acquired + Lost). Deliberately
        // does NOT cancel the epoch token: in all-serve, LeadershipLost is a
        // routine event (the loop stops leader services and continues) and the
        // token is not resettable — a watcher-driven cancel here would
        // permanently kill the caches task of a replica that briefly held
        // leadership during startup. Cleanup-only cancellation in this mode.
        // Poll at 100 ms so leader transitions are detected promptly.
        let lh = leader_handle.clone();
        let tx_leader = event_tx.clone();
        watchers.spawn("leadership", async move {
            let mut current_leader = lh.is_leader();
            if current_leader && tx_leader.send(LifecycleEvent::LeadershipAcquired).await.is_err() {
                return;
            }
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let now_leader = lh.is_leader();
                if now_leader != current_leader {
                    current_leader = now_leader;
                    let event = if now_leader {
                        LifecycleEvent::LeadershipAcquired
                    } else {
                        LifecycleEvent::LeadershipLost
                    };
                    if tx_leader.send(event).await.is_err() {
                        break;
                    }
                }
            }
        });

        // 7. Reload-signal watcher
        let reload_fut = self.reload_watcher_future(event_tx);
        watchers.spawn("reload", reload_fut);

        Ok((watchers, config_sync_server))
    }

    /// The single source of truth for which services are leader-only.
    ///
    /// NOTE: `start_conf_sync_rotation` is not merely "start a worker" — it also sets the
    /// module-global `LEADER` flag (`conf_sync_ca/rotation.rs`) that gates the Admin
    /// API's manual-rotation endpoint (`decide_trigger` / `trigger_manual_rotation`). Do
    /// not reorder or conditionally skip it without accounting for that.
    fn start_leader_jobs(client: &Client, watch_namespaces: &[String]) {
        crate::services::acme::start_acme_service(client.clone(), watch_namespaces.to_vec());
        crate::services::conf_sync_ca::rotation::start_conf_sync_rotation(client.clone());
        crate::services::dns_resolver::start_dns_resolver_service(client.clone(), watch_namespaces.to_vec());
    }

    /// Stop counterpart of [`Self::start_leader_jobs`]. All stops are no-op-if-absent.
    fn stop_leader_jobs() {
        crate::services::acme::stop_acme_service();
        crate::services::conf_sync_ca::rotation::stop_conf_sync_rotation();
        crate::services::dns_resolver::stop_dns_resolver_service();
    }

    /// Start leader-only services (ACME + status reconciliation)
    ///
    /// The `requeue_all` below has no counterpart on the leader-only path
    /// (`run_caches_ready`) by design: all-serve runs the controllers on
    /// followers too (status writes gated on leadership), so a newly promoted
    /// replica has warm caches and needs a forced reconcile to write the statuses
    /// it previously dropped. Leader-only starts controllers only after winning,
    /// and `runtime.reset()` clears the registry on every exit, so every key is
    /// reconciled from scratch as leader anyway.
    async fn start_leader_services(&self, client: &Client) {
        tracing::info!(component = "kubernetes_center", "Starting leader-only services");
        Self::start_leader_jobs(client, &self.config.watch_namespaces);
        self.runtime.processor_registry.requeue_all().await;
    }

    /// Stop leader-only services
    async fn stop_leader_services(&self) {
        tracing::info!(component = "kubernetes_center", "Stopping leader-only services");
        Self::stop_leader_jobs();
    }
}

// CenterApi implementation - delegates to `writer` via macro
impl_center_api_delegate!(KubernetesCenter, writer);

// ============================================================================
// CenterLifeCycle implementation - Kubernetes lifecycle logic
// ============================================================================

#[async_trait]
impl CenterLifeCycle for KubernetesCenter {
    /// K8s mode lifecycle with external shutdown handle
    ///
    /// This is the top-level lifecycle method for Kubernetes mode.
    /// It handles leader election and delegates to main flow when leadership is acquired.
    ///
    /// The shutdown_handle is provided by the caller (main program) to enable
    /// coordinated graceful shutdown across all components.
    async fn start(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        self.set_shutdown_handle(shutdown_handle.clone());

        // Inject conf_writer into runtime so handlers can auto-create resources (e.g. EdgionConfigData).
        let writer_arc: Arc<dyn CenterApi + Send + Sync> =
            Arc::new(KubernetesStorage::new(self.config.watch_namespaces.clone()).await?);
        self.runtime.set_conf_writer(writer_arc);

        let client = Client::try_default().await?;
        tracing::info!(
            component = "kubernetes_center",
            mode = "kubernetes",
            ha_mode = %self.config.ha_mode(),
            "K8s client initialized"
        );

        let leader_config = self.create_leader_election_config()?;
        let leader_election = LeaderElection::new(client.clone(), leader_config, self.leader_flag.clone());
        let leader_handle = leader_election.handle();

        // Fail-fast on startup if leader-election prerequisites are missing
        // (lease access / pod label patch permissions).
        leader_election.preflight_check().await?;

        let le = leader_election.clone();
        let le_shutdown = shutdown_handle.signal();
        tokio::spawn(async move {
            if let Err(e) = le.run(Some(le_shutdown)).await {
                tracing::error!(
                    component = "kubernetes_center",
                    error = %e,
                    "Leader election task failed"
                );
            }
        });

        match self.config.ha_mode() {
            HaMode::LeaderOnly => {
                self.run_leader_only_lifecycle(&client, &leader_handle, &shutdown_handle)
                    .await
            }
            HaMode::AllServe => {
                self.run_all_serve_lifecycle(&client, &leader_handle, &shutdown_handle)
                    .await
            }
        }
    }

    /// Check if the system is ready
    fn is_ready(&self) -> bool {
        self.runtime.processor_registry.is_all_ready() && self.config_sync_server.read().is_some()
    }

    /// Get the ConfigSyncServer (may be None if not ready)
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        self.config_sync_server.read().clone()
    }

    /// Check if running in Kubernetes mode
    fn is_k8s_mode(&self) -> bool {
        true
    }

    /// Kubernetes DOES have a leader election, so this returns `Some` even
    /// though `leadership_status()` stays `NotApplicable` here. The two signals
    /// intentionally disagree — see `CenterLifeCycle::leader_signal`.
    fn leader_signal(&self) -> Option<watch::Receiver<bool>> {
        Some(self.leader_flag.subscribe())
    }

    /// Request a reload (re-initialize all processors and stores)
    fn request_reload(&self) -> Result<(), String> {
        if let Some(tx) = self.reload_tx.lock().as_ref() {
            tx.try_send(())
                .map_err(|e| format!("Failed to send reload signal: {}", e))
        } else {
            Err("Center not started or not ready for reload".to_string())
        }
    }

    fn runtime(&self) -> Arc<ControllerRuntime> {
        self.runtime.clone()
    }
}

// KubernetesCenter automatically implements ConfCenter via blanket impl
// because it implements both CenterApi and CenterLifeCycle

#[cfg(test)]
mod leadership_signal_tests {
    use super::*;
    use crate::conf_mgr::conf_center::traits::LeadershipStatus;

    /// Build a `KubernetesCenter` without touching a cluster. Only the
    /// leadership signal is exercised, which never talks to the apiserver.
    fn unconnected_center() -> KubernetesCenter {
        KubernetesCenter {
            config: KubernetesConfig::new("edgion"),
            writer: KubernetesStorage::new_unconnected_for_test(),
            config_sync_server: RwLock::new(None),
            shutdown_handle: Mutex::new(None),
            reload_tx: Mutex::new(None),
            runtime: Arc::new(ControllerRuntime::new()),
            leader_flag: Arc::new(watch::channel(false).0),
        }
    }

    /// The two leadership signals deliberately DISAGREE in Kubernetes mode.
    ///
    /// `leadership_status()` must stay `NotApplicable` — it drives the Admin API
    /// not-leader 409 gate, and Kubernetes routes admin traffic to the leader
    /// via the `edgion.io/leader` pod-label Service selector while `all-serve`
    /// followers must keep serving reads. `leader_signal()` must nevertheless be
    /// `Some`, because the federation client is a process-wide singleton keyed
    /// by a replica-invariant `controller_id`.
    ///
    /// "Unifying" the two would either 409 every follower request or let every
    /// replica fight over the federation session. This test exists to make that
    /// mistake fail loudly.
    #[tokio::test]
    async fn leadership_status_is_not_applicable_while_leader_signal_is_some() {
        let center = unconnected_center();
        assert_eq!(
            center.leadership_status(),
            LeadershipStatus::NotApplicable,
            "K8s must keep the Admin API not-leader gate disabled"
        );
        assert!(
            center.leader_signal().is_some(),
            "K8s must still expose a live leadership signal for the federation client"
        );
    }

    /// The signal exists from `new()` onward and tracks the shared sender, so
    /// the federation supervisor can never subscribe before the signal exists.
    #[tokio::test]
    async fn leader_signal_tracks_the_shared_flag() {
        let center = unconnected_center();
        let mut rx = center.leader_signal().expect("signal present at construction");
        assert!(!*rx.borrow_and_update(), "a fresh center starts as a follower");

        center.leader_flag.send_replace(true);
        assert!(*rx.borrow(), "promotion must be visible to subscribers");
    }
}

#[cfg(test)]
mod caches_ready_tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn pre_cancelled_token_suppresses_caches_ready() {
        let (tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let runtime = Arc::new(ControllerRuntime::default());
        let css = Arc::new(ConfigSyncServer::new());
        // No expected kinds → readiness poll passes immediately; the cancelled
        // token must stop the tail before CachesReady is sent.
        KubernetesCenter::run_caches_ready(css, runtime, Vec::new(), Vec::new(), tx, None, Vec::new(), cancel).await;
        assert!(
            rx.try_recv().is_err(),
            "cancelled caches task must send neither CachesReady nor CachesTimeout"
        );
    }
}

#[cfg(test)]
mod failure_budget_tests {
    use super::*;

    #[test]
    fn backoff_grows_then_gives_up_after_max() {
        let mut budget = FailureBudget::new();
        let mut last = Duration::ZERO;
        for i in 1..=MAX_CONSECUTIVE_FAILURES {
            match budget.on_failure(false) {
                FailureAction::Retry(d) => {
                    assert!(d >= last, "backoff must be non-decreasing (failure {i})");
                    last = d;
                }
                FailureAction::GiveUp => panic!("gave up too early at failure {i}"),
            }
        }
        assert!(matches!(budget.on_failure(false), FailureAction::GiveUp));
    }

    #[test]
    fn stable_run_resets_the_counter() {
        let mut budget = FailureBudget::new();
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            let _ = budget.on_failure(false);
        }
        // One stable run → the next failure counts as the first again.
        assert!(matches!(budget.on_failure(true), FailureAction::Retry(_)));
        assert!(matches!(budget.on_failure(false), FailureAction::Retry(_)));
    }

    #[test]
    fn benign_reset_clears_the_counter() {
        let mut budget = FailureBudget::new();
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            let _ = budget.on_failure(false);
        }
        budget.reset();
        assert!(matches!(budget.on_failure(false), FailureAction::Retry(_)));
    }
}

#[cfg(test)]
mod loop_turn_tests {
    use super::*;
    use std::pin::pin;
    use tokio_util::sync::CancellationToken;

    fn new_set() -> WatcherSet {
        WatcherSet::new(CancellationToken::new())
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_panic_is_attributed_by_name() {
        let (_tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
        let mut watchers = new_set();
        watchers.spawn("boomer", async { panic!("boom") });
        let mut deadline = pin!(tokio::time::sleep(Duration::from_secs(90)));
        let turn = next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), true).await;
        assert!(matches!(turn, LoopTurn::WatcherPanicked("boomer")));
    }

    #[tokio::test(start_paused = true)]
    async fn event_sent_before_return_is_not_lost() {
        let (tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
        let mut watchers = new_set();
        watchers.spawn("oneshot", async move {
            let _ = tx.send(LifecycleEvent::CachesReady).await;
        });
        // Drive turns until the event arrives; a WatcherFinished in between is
        // fine (either order is legal), but the event must never be dropped.
        let mut saw_event = false;
        for _ in 0..3 {
            let mut deadline = pin!(tokio::time::sleep(Duration::from_secs(90)));
            match next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), true).await {
                LoopTurn::Event(LifecycleEvent::CachesReady) => {
                    saw_event = true;
                    break;
                }
                LoopTurn::WatcherFinished("oneshot") => continue,
                other => panic!("unexpected turn: {:?}", std::mem::discriminant(&other)),
            }
        }
        assert!(saw_event);
    }

    #[tokio::test(start_paused = true)]
    async fn armed_deadline_fires_when_nothing_happens() {
        let (_tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
        let mut watchers = new_set();
        watchers.spawn("idle", async { std::future::pending::<()>().await });
        let mut deadline = pin!(tokio::time::sleep(Duration::from_secs(90)));
        let turn = next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), true).await;
        assert!(matches!(turn, LoopTurn::DeadlineExpired));
    }

    #[tokio::test(start_paused = true)]
    async fn disarmed_deadline_never_fires() {
        let (tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
        let mut watchers = new_set();
        watchers.spawn("idle", async { std::future::pending::<()>().await });
        // Deadline already elapsed but disarmed — the turn must wait for the event.
        let mut deadline = pin!(tokio::time::sleep(Duration::from_millis(1)));
        tokio::time::sleep(Duration::from_millis(5)).await;
        let sender = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(600)).await;
            let _ = sender.send(LifecycleEvent::ReloadRequested).await;
        });
        let turn = next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), false).await;
        assert!(matches!(turn, LoopTurn::Event(LifecycleEvent::ReloadRequested)));
    }

    #[tokio::test(start_paused = true)]
    async fn closed_channel_is_reported() {
        let (tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
        drop(tx);
        let mut watchers = new_set();
        let mut deadline = pin!(tokio::time::sleep(Duration::from_secs(90)));
        let turn = next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), true).await;
        assert!(matches!(turn, LoopTurn::ChannelClosed));
    }
}
