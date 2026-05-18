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
use crate::core::controller::conf_mgr::conf_center::traits::{
    impl_center_api_delegate, CenterApi, CenterLifeCycle, ConfWriterError, ListOptions, ListResult, ResourceKey,
};
use crate::core::controller::conf_mgr::sync_runtime::metrics::reload_metrics;
use crate::core::controller::conf_mgr::sync_runtime::ShutdownHandle;
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::core::controller::conf_sync::conf_server::ConfigSyncServer;
use anyhow::Result;
use async_trait::async_trait;
use kube::Client;
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Exit reason from main flow
enum MainFlowExit {
    /// Normal shutdown requested (Ctrl+C, SIGTERM)
    Shutdown,
    /// Lost leadership, need to wait for re-election
    LostLeadership,
}

/// Lifecycle event for event-driven architecture
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

/// Exit reason from all-serve serving flow
enum ServingFlowExit {
    /// Normal shutdown requested
    Shutdown,
    /// Relink requested (410 GONE or manual reload)
    RelinkRequested,
}

/// Handles for all watcher tasks
struct WatcherHandles {
    controller: JoinHandle<()>,
    caches: JoinHandle<()>,
    leader: JoinHandle<()>,
    reload: JoinHandle<()>,
}

impl WatcherHandles {
    /// Abort all watcher tasks and wait for them to finish
    async fn abort_and_wait(self) {
        self.controller.abort();
        self.caches.abort();
        self.leader.abort();
        self.reload.abort();

        let _ = self.controller.await;
        let _ = self.caches.await;
        let _ = self.leader.await;
        let _ = self.reload.await;
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
}

impl KubernetesCenter {
    /// Create a new KubernetesCenter
    pub async fn new(config: KubernetesConfig) -> Result<Self> {
        tracing::info!(
            component = "kubernetes_center",
            mode = "kubernetes",
            gateway_class = %config.gateway_class(),
            "Creating KubernetesCenter"
        );

        let writer = KubernetesStorage::new().await?;

        Ok(Self {
            config,
            writer,
            config_sync_server: RwLock::new(None),
            shutdown_handle: Mutex::new(None),
            reload_tx: Mutex::new(None),
            runtime: Arc::new(ControllerRuntime::new()),
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

    /// Create K8s controller (new architecture - no ConfigServer dependency)
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
        const MAX_CONSECUTIVE_FAILURES: u32 = 10;
        const STABLE_RUN_DURATION: Duration = Duration::from_secs(300); // 5 minutes

        let mut consecutive_failures: u32 = 0;
        let mut reload_start_time: Option<std::time::Instant> = None;

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
                consecutive_failures = consecutive_failures,
                "Starting event watchers"
            );

            // Create event channel
            let (event_tx, mut event_rx) = mpsc::channel::<LifecycleEvent>(32);

            // Start all event watcher tasks
            let (watchers, config_sync_server) = match self
                .start_event_watchers(client, shutdown_handle, leader_handle, event_tx)
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!(
                        component = "kubernetes_center",
                        mode = "kubernetes",
                        error = %e,
                        "Failed to start event watchers"
                    );
                    consecutive_failures += 1;
                    if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                        tracing::error!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            consecutive_failures = consecutive_failures,
                            "Max consecutive failures exceeded, giving up"
                        );
                        return MainFlowExit::Shutdown;
                    }
                    tokio::time::sleep(Self::backoff(consecutive_failures)).await;
                    continue;
                }
            };

            let iteration_start = Instant::now();
            let mut caches_ready = false;

            // Event-driven main loop - no select, just simple match
            let exit_reason = loop {
                match event_rx.recv().await {
                    Some(LifecycleEvent::CachesReady) => {
                        caches_ready = true;
                        self.set_config_sync_server(Some(config_sync_server.clone()));
                        tracing::info!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            "ConfigSyncServer is ready, gRPC services can process requests"
                        );
                    }
                    Some(LifecycleEvent::CachesTimeout) => {
                        tracing::error!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            "Caches timeout, treating as controller failure"
                        );
                        break ControllerExitReason::AllControllersStopped;
                    }
                    Some(LifecycleEvent::LeadershipLost) => {
                        tracing::warn!(component = "kubernetes_center", mode = "kubernetes", "Lost leadership");
                        break ControllerExitReason::LostLeadership;
                    }
                    Some(LifecycleEvent::LeadershipAcquired) => {
                        // Not expected in leader-only mode, ignore
                    }
                    Some(LifecycleEvent::ControllerExit(reason)) => {
                        tracing::info!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            reason = ?reason,
                            "Controller exited"
                        );
                        break reason;
                    }
                    Some(LifecycleEvent::ReloadRequested) => {
                        tracing::info!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            "Reload requested via Admin API, restarting controllers"
                        );
                        break ControllerExitReason::RelinkRequested(
                            super::resource_controller::RelinkReason::ReloadRequested,
                        );
                    }
                    None => {
                        tracing::error!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            "Event channel closed unexpectedly"
                        );
                        break ControllerExitReason::AllControllersStopped;
                    }
                }
            };

            // Cleanup: abort and wait for all watcher tasks
            watchers.abort_and_wait().await;

            // Clear config_sync_server if it was set
            if caches_ready {
                self.set_config_sync_server(None);
            }

            // Stop ACME service and reset ControllerRuntime for retry
            crate::core::controller::services::acme::stop_acme_service();
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
                    consecutive_failures = 0;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                ControllerExitReason::AllControllersStopped => {
                    // Real error - count as failure and apply backoff
                    tracing::warn!(
                        component = "kubernetes_center",
                        mode = "kubernetes",
                        "All controllers stopped, will restart with backoff"
                    );

                    // Reset failure counter if ran stably for long enough
                    if Instant::now().saturating_duration_since(iteration_start) >= STABLE_RUN_DURATION {
                        consecutive_failures = 0;
                    }

                    consecutive_failures += 1;
                    if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                        tracing::error!(
                            component = "kubernetes_center",
                            mode = "kubernetes",
                            consecutive_failures = consecutive_failures,
                            "Max consecutive failures exceeded, giving up"
                        );
                        return MainFlowExit::Shutdown;
                    }

                    tracing::info!(
                        component = "kubernetes_center",
                        mode = "kubernetes",
                        backoff_secs = Self::backoff(consecutive_failures).as_secs(),
                        consecutive_failures = consecutive_failures,
                        "Waiting before restart"
                    );
                    tokio::time::sleep(Self::backoff(consecutive_failures)).await;
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
    /// - `WatcherHandles`: Handles to abort/await all watcher tasks
    /// - `Arc<ConfigSyncServer>`: The config sync server for setting service availability
    async fn start_event_watchers(
        &self,
        client: &Client,
        shutdown_handle: &ShutdownHandle,
        leader_handle: &LeaderHandle,
        event_tx: mpsc::Sender<LifecycleEvent>,
    ) -> Result<(WatcherHandles, Arc<ConfigSyncServer>)> {
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

        // 5. Spawn controller.run task
        let shutdown_signal = shutdown_handle.signal();
        let tx = event_tx.clone();
        let runtime_for_controller = self.runtime.clone();
        let controller_handle = tokio::spawn(async move {
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

        // 6. Spawn caches-ready watcher (starts ACME on ready — leader-only mode)
        let no_sync_kinds = crate::core::common::config::get_no_sync_kinds();
        let caches_handle = Self::spawn_caches_ready_task(
            config_sync_server.clone(),
            self.runtime.clone(),
            expected_processor_kinds,
            no_sync_kinds,
            event_tx.clone(),
            Some(client.clone()), // start ACME when ready
        );

        // 7. Spawn leadership-loss watcher (leader-only: only fires LeadershipLost)
        let lh = leader_handle.clone();
        let tx = event_tx.clone();
        let leader_watcher_handle = tokio::spawn(async move {
            while lh.is_leader() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            let _ = tx.send(LifecycleEvent::LeadershipLost).await;
        });

        // 8. Spawn reload-signal watcher
        let reload_watcher_handle = self.spawn_reload_watcher(event_tx);

        Ok((
            WatcherHandles {
                controller: controller_handle,
                caches: caches_handle,
                leader: leader_watcher_handle,
                reload: reload_watcher_handle,
            },
            config_sync_server,
        ))
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
                    crate::core::controller::services::acme::stop_acme_service();
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
                    crate::core::controller::services::acme::stop_acme_service();
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

    /// Spawn caches-ready watcher task.
    ///
    /// Polls until all expected processor kinds are registered and synced,
    /// then registers all WatchObjs, triggers cross-namespace revalidation,
    /// and fires `CachesReady`. On timeout fires `CachesTimeout`.
    ///
    /// `acme_client`: if `Some`, starts the ACME service after caches are ready
    /// (leader-only mode). Pass `None` for all-serve (ACME starts via `start_leader_services`).
    fn spawn_caches_ready_task(
        css: Arc<ConfigSyncServer>,
        runtime: Arc<ControllerRuntime>,
        expected_processor_kinds: Vec<&'static str>,
        no_sync_kinds: Vec<String>,
        tx: mpsc::Sender<LifecycleEvent>,
        acme_client: Option<Client>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
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
                css.register_all(registry.all_watch_objs(&no_sync_refs));

                crate::core::controller::conf_mgr::sync_runtime::resource_processor::run_post_cache_ready_revalidations(
                    runtime.clone(),
                )
                .await;

                if let Some(client) = acme_client {
                    crate::core::controller::services::acme::start_acme_service(client);
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
        })
    }

    /// Spawn reload-signal watcher task. Returns the task handle.
    fn spawn_reload_watcher(&self, tx: mpsc::Sender<LifecycleEvent>) -> JoinHandle<()> {
        let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);
        self.set_reload_tx(Some(reload_tx));
        tokio::spawn(async move {
            if reload_rx.recv().await.is_some() {
                let _ = tx.send(LifecycleEvent::ReloadRequested).await;
            }
        })
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
        loop {
            let exit = self.run_serving_flow(client, leader_handle, shutdown_handle).await;
            match exit {
                ServingFlowExit::Shutdown => return Ok(()),
                ServingFlowExit::RelinkRequested => {
                    tracing::info!(component = "kubernetes_center", "ControllerRuntime reset");
                    self.runtime.reset();
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
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
        let (event_tx, mut event_rx) = mpsc::channel::<LifecycleEvent>(32);

        let (watchers, config_sync_server) = match self
            .start_event_watchers_all_serve(client, shutdown_handle, leader_handle, event_tx)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(
                    component = "kubernetes_center",
                    error = %e,
                    "Failed to start event watchers (all-serve)"
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
                return ServingFlowExit::RelinkRequested;
            }
        };

        let mut caches_ready = false;
        let mut leader_services_running = false;

        let exit_reason = loop {
            match event_rx.recv().await {
                Some(LifecycleEvent::CachesReady) => {
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
                Some(LifecycleEvent::LeadershipAcquired) => {
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
                Some(LifecycleEvent::LeadershipLost) => {
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
                Some(LifecycleEvent::CachesTimeout) => {
                    break ControllerExitReason::AllControllersStopped;
                }
                Some(LifecycleEvent::ControllerExit(reason)) => {
                    break reason;
                }
                Some(LifecycleEvent::ReloadRequested) => {
                    break ControllerExitReason::RelinkRequested(
                        super::resource_controller::RelinkReason::ReloadRequested,
                    );
                }
                None => {
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
            _ => ServingFlowExit::RelinkRequested,
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
    ) -> Result<(WatcherHandles, Arc<ConfigSyncServer>)> {
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

        // 5. Spawn controller.run task
        let shutdown_signal = shutdown_handle.signal();
        let tx = event_tx.clone();
        let runtime_for_controller = self.runtime.clone();
        let controller_handle = tokio::spawn(async move {
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

        // 6. Spawn caches-ready watcher (no ACME — started via start_leader_services)
        let no_sync_kinds = crate::core::common::config::get_no_sync_kinds();
        let caches_handle = Self::spawn_caches_ready_task(
            config_sync_server.clone(),
            self.runtime.clone(),
            expected_processor_kinds,
            no_sync_kinds,
            event_tx.clone(),
            None, // ACME starts via start_leader_services
        );

        // 7. Spawn leadership watcher — bidirectional (Acquired + Lost)
        // Poll at 100 ms so leader transitions are detected promptly.
        let lh = leader_handle.clone();
        let tx_leader = event_tx.clone();
        let leader_watcher_handle = tokio::spawn(async move {
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

        // 8. Spawn reload-signal watcher
        let reload_watcher_handle = self.spawn_reload_watcher(event_tx);

        Ok((
            WatcherHandles {
                controller: controller_handle,
                caches: caches_handle,
                leader: leader_watcher_handle,
                reload: reload_watcher_handle,
            },
            config_sync_server,
        ))
    }

    /// Start leader-only services (ACME + status reconciliation)
    async fn start_leader_services(&self, client: &Client) {
        tracing::info!(component = "kubernetes_center", "Starting leader-only services");
        crate::core::controller::services::acme::start_acme_service(client.clone());
        self.runtime.processor_registry.requeue_all().await;
    }

    /// Stop leader-only services
    async fn stop_leader_services(&self) {
        tracing::info!(component = "kubernetes_center", "Stopping leader-only services");
        crate::core::controller::services::acme::stop_acme_service();
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

        // Inject conf_writer into runtime so handlers can auto-create resources (e.g. PluginMetaData).
        let writer_arc: Arc<dyn CenterApi + Send + Sync> = Arc::new(KubernetesStorage::new().await?);
        self.runtime.set_conf_writer(writer_arc);

        let client = Client::try_default().await?;
        tracing::info!(
            component = "kubernetes_center",
            mode = "kubernetes",
            ha_mode = %self.config.ha_mode(),
            "K8s client initialized"
        );

        let leader_config = self.create_leader_election_config()?;
        let leader_election = LeaderElection::new(client.clone(), leader_config);
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
