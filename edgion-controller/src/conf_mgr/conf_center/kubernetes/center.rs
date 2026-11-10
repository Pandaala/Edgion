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
use super::version_detection::{
    preflight_check_gateway_api_crds, validate_no_watch_kinds, LegacyRouteVersion, LEGACY_SOURCE_KINDS,
};
use crate::conf_mgr::conf_center::traits::{
    impl_center_api_delegate, CenterApi, CenterLifeCycle, ConfWriterError, ListOptions, ListResult, ResourceKey,
};
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
use tokio_util::task::TaskTracker;

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

    /// Benign leadership transition resets the failure budget.
    fn reset(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Record a failure exit. `ran_stably` = the iteration reached CachesReady
    /// and then ran at least STABLE_RUN_DURATION before failing
    /// (reset-before-count). See [`ran_stably`].
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

/// Whether a failed iteration counts as a stable run that resets the failure budget.
///
/// Only time spent serving after CachesReady counts. Time spent waiting for
/// readiness never does, so a `cache_ready_deadline_secs` at or above
/// `STABLE_RUN_DURATION` cannot turn every startup-deadline expiry into a
/// budget reset and disable give-up.
fn ran_stably(ready_since: Option<Instant>, now: Instant) -> bool {
    ready_since.is_some_and(|ready| now.saturating_duration_since(ready) >= STABLE_RUN_DURATION)
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
    /// Lost leadership
    LeadershipLost,
    /// Acquired leadership (all-serve mode only)
    LeadershipAcquired,
    /// Controller exited
    ControllerExit(ControllerExitReason),
}

/// Exit reason from all-serve serving flow.
enum ServingFlowExit {
    /// Normal shutdown requested
    Shutdown,
    /// Benign leadership transition — resets the failure budget
    Benign,
    /// Real failure (watcher panic / deadline / channel closed / controller stop) — counts toward
    /// give-up. `ran_stably` = the flow reached CachesReady and then served for
    /// at least `STABLE_RUN_DURATION` before failing.
    Failure { ran_stably: bool },
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
    tracker: TaskTracker,
}

impl WatcherSet {
    fn new(cancel: CancellationToken) -> Self {
        Self {
            set: JoinSet::new(),
            names: HashMap::new(),
            cancel,
            tracker: TaskTracker::new(),
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
        if tokio::time::timeout(Duration::from_secs(5), async {
            while self.set.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            self.set.abort_all();
            while self.set.join_next().await.is_some() {}
        }
        // All top-level spawners are joined. Tracked parents retain membership
        // until their synchronously registered children have been spawned.
        self.tracker.close();
        self.tracker.wait().await;
    }
}

fn classify_leader_only_exit(
    reason: ControllerExitReason,
    shutdown: bool,
    leader: bool,
    epoch_cancelled: bool,
) -> ControllerExitReason {
    if shutdown {
        ControllerExitReason::Shutdown
    } else if !leader || epoch_cancelled {
        ControllerExitReason::LostLeadership
    } else {
        reason
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
    controller_name: String,
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
        let controller_name = config.resolve_controller_name()?;

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
            "Creating KubernetesCenter"
        );

        let writer = KubernetesStorage::new(config.watch_namespaces.clone()).await?;

        let runtime = Arc::new(ControllerRuntime::new());
        runtime.set_single_publisher(config.ha_mode() != HaMode::AllServe);

        Ok(Self {
            controller_name,
            config,
            writer,
            config_sync_server: RwLock::new(None),
            shutdown_handle: Mutex::new(None),
            runtime,
            leader_flag: Arc::new(watch::channel(false).0),
        })
    }

    /// Get the configuration
    pub fn config(&self) -> &KubernetesConfig {
        &self.config
    }

    // ==================== Helper Methods ====================

    /// Set the ConfigSyncServer (Some = ready, None = not ready)
    async fn stop_resource_epoch(&self, watchers: WatcherSet) {
        // Withdraw publication before a possibly unbounded descendant drain.
        self.set_config_sync_server(None);
        watchers.abort_and_wait().await;
    }

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
    fn create_k8s_controller(
        &self,
        client: &Client,
        reference_grant_version: Option<super::version_detection::ReferenceGrantVersion>,
        gateway_class_version: Option<super::version_detection::GatewayClassVersion>,
        gateway_version: Option<super::version_detection::GatewayVersion>,
        http_route_version: Option<super::version_detection::HTTPRouteVersion>,
        legacy_versions: HashMap<&'static str, LegacyRouteVersion>,
    ) -> Result<KubernetesController> {
        let config = self.config();

        tracing::info!(
            component = "kubernetes_center",
            mode = "kubernetes",
            controller_name = %self.controller_name,
            namespaces = ?config.watch_namespaces(),
            metadata_filter_enabled = true,
            blocked_annotations_count = config.metadata_filter().blocked_annotations.len(),
            remove_managed_fields = config.metadata_filter().remove_managed_fields,
            "Creating Kubernetes controller"
        );

        let no_watch_kinds = config.no_watch_kinds.iter().cloned().collect();
        KubernetesController::with_metadata_filter(
            client.clone(),
            self.controller_name.clone(),
            config.gateway_address.clone(),
            config.watch_namespaces.clone(),
            config.label_selector.clone(),
            config.metadata_filter.clone(),
            no_watch_kinds,
        )
        .map(|controller| {
            controller
                .with_reference_grant_version(reference_grant_version)
                .with_gateway_class_version(gateway_class_version)
                .with_gateway_version(gateway_version)
                .with_http_route_version(http_route_version)
                .with_legacy_versions(legacy_versions)
        })
    }

    async fn selected_legacy_versions(
        &self,
        no_watch_kinds: &std::collections::HashSet<String>,
    ) -> Result<HashMap<&'static str, LegacyRouteVersion>> {
        let mut selected = HashMap::new();
        for (kind, _) in LEGACY_SOURCE_KINDS {
            if !no_watch_kinds.contains(kind) {
                if let Some(version) = self.writer.selected_legacy_version(kind).await? {
                    selected.insert(kind, version);
                }
            }
        }
        Ok(selected)
    }

    /// Main flow - runs only when this instance is the leader
    ///
    /// Event-driven architecture:
    /// 1. Start all event watcher tasks (controller, caches, leadership)
    /// 2. Process events in a simple match loop
    /// 3. Handle retry logic for terminal resource-epoch errors
    async fn run_main_flow(
        &self,
        client: &Client,
        leader_handle: &LeaderHandle,
        shutdown_handle: &ShutdownHandle,
    ) -> MainFlowExit {
        let mut budget = FailureBudget::new();
        let deadline_budget = Duration::from_secs(self.config.cache_ready_deadline_secs);

        loop {
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

            let mut caches_ready = false;
            let mut ready_since: Option<Instant> = None;
            let mut deadline = Box::pin(tokio::time::sleep(deadline_budget));

            // Supervised event loop: events / watcher joins / pre-ready deadline.
            let exit_reason = loop {
                let turn = next_loop_turn(&mut event_rx, &mut watchers, deadline.as_mut(), !caches_ready).await;
                match turn {
                    LoopTurn::Event(event) => match event {
                        LifecycleEvent::CachesReady => {
                            caches_ready = true;
                            ready_since = Some(Instant::now());
                            self.set_config_sync_server(Some(config_sync_server.clone()));
                            tracing::info!(
                                component = "kubernetes_center",
                                mode = "kubernetes",
                                "ConfigSyncServer is ready, gRPC services can process requests"
                            );
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

            // Epoch cancellation can finish the controller before LeadershipLost
            // reaches this channel. Process shutdown and leadership are authoritative.
            let exit_reason = classify_leader_only_exit(
                exit_reason,
                shutdown_handle.is_shutdown(),
                leader_handle.is_leader(),
                watchers.cancel.is_cancelled(),
            );
            self.stop_resource_epoch(watchers).await;

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
                ControllerExitReason::AllControllersStopped => {
                    // Real error - count as failure and apply backoff
                    tracing::warn!(
                        component = "kubernetes_center",
                        mode = "kubernetes",
                        "All controllers stopped, will restart with backoff"
                    );

                    match budget.on_failure(ran_stably(ready_since, Instant::now())) {
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
        let reference_grant_version = if no_watch_kinds.contains("ReferenceGrant") {
            None
        } else {
            Some(self.writer.selected_reference_grant_version().await?)
        };
        let gateway_class_version = if no_watch_kinds.contains("GatewayClass") {
            None
        } else {
            Some(self.writer.selected_gateway_class_version().await?)
        };
        let gateway_version = if no_watch_kinds.contains("Gateway") {
            None
        } else {
            Some(self.writer.selected_gateway_version().await?)
        };
        let http_route_version = if no_watch_kinds.contains("HTTPRoute") {
            None
        } else {
            Some(self.writer.selected_http_route_version().await?)
        };
        let legacy_versions = self.selected_legacy_versions(&no_watch_kinds).await?;
        preflight_check_gateway_api_crds(
            client,
            &no_watch_kinds,
            reference_grant_version,
            gateway_class_version,
            gateway_version,
            http_route_version,
        )
        .await?;

        // 2. Create ConfigSyncServer (will be populated when caches are ready)
        let config_sync_server = Arc::new(ConfigSyncServer::new());

        // 3. Create Controller and leader handle
        let controller = self
            .create_k8s_controller(
                client,
                reference_grant_version,
                gateway_class_version,
                gateway_version,
                http_route_version,
                legacy_versions,
            )?
            .with_leader_handle(leader_handle.clone());
        let expected_processor_kinds = controller.all_processor_kinds();

        let mut watchers = WatcherSet::new(CancellationToken::new());
        let (phases_ready_tx, phases_ready) = watch::channel(false);
        let controller = controller
            .with_epoch(watchers.cancel_token(), watchers.tracker.clone())
            .with_phases_ready(phases_ready_tx);

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
                Some(leader_handle.clone()),
                self.config.watch_namespaces.clone(),
                watchers.cancel_token(),
                phases_ready,
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
        leader_handle: Option<LeaderHandle>,
        watch_namespaces: Vec<String>,
        cancel: CancellationToken,
        mut phases_ready: watch::Receiver<bool>,
    ) {
        // Namespace readiness is not represented by a processor kind. Even when
        // Gateway/dependents are disabled, publication waits for the phase gate.
        let phase_wait = async {
            while !*phases_ready.borrow() {
                phases_ready.changed().await?;
            }
            Ok::<_, watch::error::RecvError>(())
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            result = phase_wait => if result.is_err() { return; },
        }
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

        // No inner deadline: the supervised loop's `cache_ready_deadline_secs`
        // is the single startup bound and cancels this task on expiry.
        let readiness_poll = async {
            while !pending_sync_kinds().is_empty() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = readiness_poll => {}
        }

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
        // Every expected processor is registered and ready: the Kubernetes
        // equivalent of the FS/etcd `spawn_all_controllers` mark. In-process
        // sources that requeue keys (the Controller DNS resolver) wait on it.
        registry.mark_registration_complete();
        if let (Some(client), Some(leader_handle)) = (acme_client, leader_handle) {
            Self::start_leader_jobs(&client, &watch_namespaces, runtime, leader_handle);
        }
        let _ = tx.send(LifecycleEvent::CachesReady).await;
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
        let mut shutdown_signal = shutdown_handle.signal();
        loop {
            if shutdown_handle.is_shutdown() {
                return Ok(());
            }
            let exit = self.run_serving_flow(client, leader_handle, shutdown_handle).await;
            match exit {
                ServingFlowExit::Shutdown => return Ok(()),
                ServingFlowExit::Benign => {
                    budget.reset();
                    tokio::select! {
                        _ = shutdown_signal.wait() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    }
                }
                ServingFlowExit::Failure { ran_stably } => match budget.on_failure(ran_stably) {
                    FailureAction::Retry(delay) => {
                        tracing::info!(
                            component = "kubernetes_center",
                            ha_mode = "all-serve",
                            backoff_secs = delay.as_secs(),
                            "Waiting before restart"
                        );
                        tokio::select! {
                            _ = shutdown_signal.wait() => return Ok(()),
                            _ = tokio::time::sleep(delay) => {}
                        }
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
                },
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
                return ServingFlowExit::Failure { ran_stably: false };
            }
            Err(_elapsed) => {
                tracing::error!(
                    component = "kubernetes_center",
                    deadline_secs = deadline_budget.as_secs(),
                    "start_event_watchers exceeded cache_ready_deadline_secs (all-serve)"
                );
                return ServingFlowExit::Failure { ran_stably: false };
            }
        };

        let mut caches_ready = false;
        let mut ready_since: Option<Instant> = None;
        let mut leader_services_running = false;
        let mut deadline = Box::pin(tokio::time::sleep(deadline_budget));

        // Supervised event loop: events / watcher joins / pre-ready deadline.
        let exit_reason = loop {
            let turn = next_loop_turn(&mut event_rx, &mut watchers, deadline.as_mut(), !caches_ready).await;
            match turn {
                LoopTurn::Event(event) => match event {
                    LifecycleEvent::CachesReady => {
                        caches_ready = true;
                        ready_since = Some(Instant::now());
                        self.set_config_sync_server(Some(config_sync_server.clone()));
                        tracing::info!(
                            component = "kubernetes_center",
                            ha_mode = "all-serve",
                            "ConfigSyncServer is ready"
                        );

                        if leader_handle.is_leader() && !leader_services_running {
                            self.start_leader_services(client, leader_handle).await;
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
                            self.start_leader_services(client, leader_handle).await;
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
                    LifecycleEvent::ControllerExit(reason) => {
                        break reason;
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

        let ran_stably = ran_stably(ready_since, Instant::now());
        self.finish_serving_flow(
            watchers,
            leader_services_running,
            exit_reason,
            ran_stably,
            shutdown_handle,
        )
        .await
    }

    async fn finish_serving_flow(
        &self,
        watchers: WatcherSet,
        leader_services_running: bool,
        exit_reason: ControllerExitReason,
        ran_stably: bool,
        shutdown_handle: &ShutdownHandle,
    ) -> ServingFlowExit {
        self.stop_resource_epoch(watchers).await;
        if leader_services_running {
            self.stop_leader_services().await;
        }
        tracing::info!(component = "kubernetes_center", "ControllerRuntime reset");
        self.runtime.reset();

        // A phase/child may report failure before the shutdown event reaches
        // the channel, or shutdown may arrive during drain. Never restart that epoch.
        if shutdown_handle.is_shutdown() {
            return ServingFlowExit::Shutdown;
        }
        match exit_reason {
            ControllerExitReason::Shutdown => ServingFlowExit::Shutdown,
            // LostLeadership bubbling up via ControllerExit is a leadership
            // transition, not a fault — the loop normally handles it inline.
            ControllerExitReason::LostLeadership => ServingFlowExit::Benign,
            ControllerExitReason::AllControllersStopped => ServingFlowExit::Failure { ran_stably },
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
        let reference_grant_version = if no_watch_kinds.contains("ReferenceGrant") {
            None
        } else {
            Some(self.writer.selected_reference_grant_version().await?)
        };
        let gateway_class_version = if no_watch_kinds.contains("GatewayClass") {
            None
        } else {
            Some(self.writer.selected_gateway_class_version().await?)
        };
        let gateway_version = if no_watch_kinds.contains("Gateway") {
            None
        } else {
            Some(self.writer.selected_gateway_version().await?)
        };
        let http_route_version = if no_watch_kinds.contains("HTTPRoute") {
            None
        } else {
            Some(self.writer.selected_http_route_version().await?)
        };
        let legacy_versions = self.selected_legacy_versions(&no_watch_kinds).await?;
        preflight_check_gateway_api_crds(
            client,
            &no_watch_kinds,
            reference_grant_version,
            gateway_class_version,
            gateway_version,
            http_route_version,
        )
        .await?;

        // 2. Create ConfigSyncServer
        let config_sync_server = Arc::new(ConfigSyncServer::new());

        // 3. Create controller with leader_handle for status write gating
        let controller = self
            .create_k8s_controller(
                client,
                reference_grant_version,
                gateway_class_version,
                gateway_version,
                http_route_version,
                legacy_versions,
            )?
            .with_leader_handle(leader_handle.clone());
        let expected_processor_kinds = controller.all_processor_kinds();

        let mut watchers = WatcherSet::new(CancellationToken::new());
        let (phases_ready_tx, phases_ready) = watch::channel(false);
        let controller = controller
            .with_epoch(watchers.cancel_token(), watchers.tracker.clone())
            .with_phases_ready(phases_ready_tx);

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
                None,
                self.config.watch_namespaces.clone(),
                watchers.cancel_token(),
                phases_ready,
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

        Ok((watchers, config_sync_server))
    }

    /// The single source of truth for which services are leader-only.
    ///
    /// NOTE: `start_conf_sync_rotation` is not merely "start a worker" — it also sets the
    /// module-global `LEADER` flag (`conf_sync_ca/rotation.rs`) that gates the Admin
    /// API's manual-rotation endpoint (`decide_trigger` / `trigger_manual_rotation`). Do
    /// not reorder or conditionally skip it without accounting for that.
    fn start_leader_jobs(
        client: &Client,
        watch_namespaces: &[String],
        runtime: Arc<ControllerRuntime>,
        leader_handle: LeaderHandle,
    ) {
        crate::services::acme::start_acme_service(
            client.clone(),
            watch_namespaces.to_vec(),
            runtime.clone(),
            leader_handle.clone(),
        );
        crate::services::conf_sync_ca::rotation::start_conf_sync_rotation(client.clone());
        crate::services::credential_manager::start_credential_manager_service(
            client.clone(),
            watch_namespaces.to_vec(),
            runtime,
            leader_handle,
        );
    }

    /// Stop counterpart of [`Self::start_leader_jobs`]. All stops are no-op-if-absent.
    fn stop_leader_jobs() {
        crate::services::acme::stop_acme_service();
        crate::services::conf_sync_ca::rotation::stop_conf_sync_rotation();
        crate::services::credential_manager::stop_credential_manager_service();
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
    async fn start_leader_services(&self, client: &Client, leader_handle: &LeaderHandle) {
        tracing::info!(component = "kubernetes_center", "Starting leader-only services");
        Self::start_leader_jobs(
            client,
            &self.config.watch_namespaces,
            self.runtime.clone(),
            leader_handle.clone(),
        );
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
        let writer_arc: Arc<dyn CenterApi + Send + Sync> = Arc::new(
            KubernetesStorage::new_with_selection(
                self.config.watch_namespaces.clone(),
                self.writer.selection(),
                self.writer.gateway_class_selection(),
                self.writer.gateway_selection(),
                self.writer.http_route_selection(),
            )
            .await?,
        );
        self.runtime.set_conf_writer(writer_arc);

        let client = edgion_common::kube_client::default_client().await?;
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

    fn runtime(&self) -> Arc<ControllerRuntime> {
        self.runtime.clone()
    }
}

// KubernetesCenter automatically implements ConfCenter via blanket impl
// because it implements both CenterApi and CenterLifeCycle

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/center_test/leadership_signal_tests.rs"]
mod leadership_signal_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/center_test/caches_ready_tests.rs"]
mod caches_ready_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/center_test/failure_budget_tests.rs"]
mod failure_budget_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/center_test/loop_turn_tests.rs"]
mod loop_turn_tests;
