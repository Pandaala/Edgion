//! Kubernetes mode lifecycle implementation
//!
//! This module manages the complete lifecycle for Kubernetes mode using event-driven architecture.
//!
//! ## Architecture
//!
//! ```text
//! run_k8s_lifecycle()
//! │
//! ├── Phase 1: Leader Election Setup
//! │   ├── Create K8s Client
//! │   ├── Initialize LeaderElection
//! │   └── Spawn leader election background task
//! │
//! └── Main Loop
//!     │
//!     ├── Wait for Leadership
//!     │   └── Block until this instance becomes leader
//!     │
//!     └── Main Flow (event-driven, only leader executes)
//!         │
//!         ├── Start Event Watchers (returns config_sync_server + watcher handles)
//!         │   ├── Controller task → ControllerExit event
//!         │   ├── Caches watcher → CachesReady event (monitors PROCESSOR_REGISTRY)
//!         │   └── Leadership watcher → LeadershipLost event
//!         │
//!         ├── Event Loop (simple match on event_rx.recv())
//!         │   ├── CachesReady → set_config_sync_server(Some)
//!         │   ├── LeadershipLost → Break, return LostLeadership
//!         │   └── ControllerExit → Break, handle exit reason
//!         │
//!         └── Handle Exit Reason
//!             ├── Shutdown → Exit program
//!             ├── LostLeadership → Back to "Wait for Leadership"
//!             ├── RelinkRequested (410 GONE) → Retry immediately
//!             └── AllControllersStopped → Retry with backoff
//! ```

use super::conf_center::ConfCenter;
use super::config::{ConfCenterConfig, EndpointMode};
use super::kubernetes::{
    resolve_endpoint_mode, ControllerExitReason, KubernetesController,
    LeaderElection, LeaderElectionConfig as K8sLeaderElectionConfig, LeaderHandle,
};
use crate::core::conf_mgr_new::sync_runtime::ShutdownHandle;
use crate::core::conf_mgr_new::PROCESSOR_REGISTRY;
use crate::core::conf_sync::conf_server_new::ConfigSyncServer;
use anyhow::Result;
use kube::Client;
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
    /// Controller exited
    ControllerExit(ControllerExitReason),
}

/// Handles for all watcher tasks
struct WatcherHandles {
    controller: JoinHandle<()>,
    caches: JoinHandle<()>,
    leader: JoinHandle<()>,
}

impl WatcherHandles {
    /// Abort all watcher tasks and wait for them to finish
    async fn abort_and_wait(self) {
        self.controller.abort();
        self.caches.abort();
        self.leader.abort();

        let _ = self.controller.await;
        let _ = self.caches.await;
        let _ = self.leader.await;
    }
}

impl ConfCenter {
    /// K8s mode lifecycle with external shutdown handle
    ///
    /// This is the top-level lifecycle method for Kubernetes mode.
    /// It handles leader election and delegates to main flow when leadership is acquired.
    ///
    /// The shutdown_handle is provided by the caller (main program) to enable
    /// coordinated graceful shutdown across all components.
    pub(super) async fn run_k8s_lifecycle_with_shutdown(
        &self,
        shutdown_handle: ShutdownHandle,
    ) -> Result<()> {
        // Store shutdown handle
        self.set_shutdown_handle(shutdown_handle.clone());

        // 1. Create K8s Client
        let client = Client::try_default().await?;
        tracing::info!(
            component = "conf_center_new",
            mode = "kubernetes",
            "K8s client initialized"
        );

        // 2. Initialize Leader Election
        let leader_config = self.create_leader_election_config()?;
        let leader_election = LeaderElection::new(client.clone(), leader_config);
        let leader_handle = leader_election.handle();

        // 3. Spawn leader election background task with shutdown signal
        let le = leader_election.clone();
        let le_shutdown = shutdown_handle.signal();
        tokio::spawn(async move {
            if let Err(e) = le.run(Some(le_shutdown)).await {
                tracing::error!(
                    component = "conf_center_new",
                    error = %e,
                    "Leader election task failed"
                );
            }
        });

        // 4. Main lifecycle loop
        loop {
            // === Phase 1: Wait for leadership ===
            tracing::info!(
                component = "conf_center_new",
                mode = "kubernetes",
                "Waiting for leadership..."
            );

            if !leader_handle
                .wait_until_leader_with_shutdown(shutdown_handle.signal())
                .await
            {
                tracing::info!(
                    component = "conf_center_new",
                    mode = "kubernetes",
                    "Shutdown requested before acquiring leadership"
                );
                return Ok(());
            }

            tracing::info!(
                component = "conf_center_new",
                mode = "kubernetes",
                "Acquired leadership, entering main flow"
            );

            // === Phase 2: Run main flow (only leader executes) ===
            let exit_reason = self
                .run_main_flow(&client, &leader_handle, &shutdown_handle)
                .await;

            // === Phase 3: Handle exit reason ===
            match exit_reason {
                MainFlowExit::Shutdown => {
                    tracing::info!(
                        component = "conf_center_new",
                        mode = "kubernetes",
                        "Normal shutdown"
                    );
                    // Clear PROCESSOR_REGISTRY
                    PROCESSOR_REGISTRY.clear_registry();
                    return Ok(());
                }
                MainFlowExit::LostLeadership => {
                    tracing::warn!(
                        component = "conf_center_new",
                        mode = "kubernetes",
                        "Lost leadership, will wait for re-election"
                    );
                    // Clear PROCESSOR_REGISTRY for re-election
                    PROCESSOR_REGISTRY.clear_registry();
                    // Loop back to wait for leadership
                    continue;
                }
            }
        }
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

        loop {
            // Check if still leader before starting iteration
            if !leader_handle.is_leader() {
                self.set_config_sync_server(None);
                return MainFlowExit::LostLeadership;
            }

            tracing::info!(
                component = "conf_center_new",
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
                        component = "conf_center_new",
                        mode = "kubernetes",
                        error = %e,
                        "Failed to start event watchers"
                    );
                    consecutive_failures += 1;
                    if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                        tracing::error!(
                            component = "conf_center_new",
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
                            component = "conf_center_new",
                            mode = "kubernetes",
                            "ConfigSyncServer is ready, gRPC services can process requests"
                        );
                    }
                    Some(LifecycleEvent::CachesTimeout) => {
                        tracing::error!(
                            component = "conf_center_new",
                            mode = "kubernetes",
                            "Caches timeout, treating as controller failure"
                        );
                        break ControllerExitReason::AllControllersStopped;
                    }
                    Some(LifecycleEvent::LeadershipLost) => {
                        tracing::warn!(
                            component = "conf_center_new",
                            mode = "kubernetes",
                            "Lost leadership"
                        );
                        break ControllerExitReason::LostLeadership;
                    }
                    Some(LifecycleEvent::ControllerExit(reason)) => {
                        tracing::info!(
                            component = "conf_center_new",
                            mode = "kubernetes",
                            reason = ?reason,
                            "Controller exited"
                        );
                        break reason;
                    }
                    None => {
                        tracing::error!(
                            component = "conf_center_new",
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

            // Clear PROCESSOR_REGISTRY for retry
            PROCESSOR_REGISTRY.clear_registry();

            // Handle exit reason
            match exit_reason {
                ControllerExitReason::Shutdown => {
                    return MainFlowExit::Shutdown;
                }
                ControllerExitReason::LostLeadership => {
                    return MainFlowExit::LostLeadership;
                }
                ControllerExitReason::RelinkRequested(reason) => {
                    // 410 GONE - normal reconnection, don't count as failure
                    tracing::info!(
                        component = "conf_center_new",
                        mode = "kubernetes",
                        reason = ?reason,
                        "Relink requested (410 GONE), restarting immediately"
                    );
                    consecutive_failures = 0;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                ControllerExitReason::AllControllersStopped => {
                    // Real error - count as failure and apply backoff
                    tracing::warn!(
                        component = "conf_center_new",
                        mode = "kubernetes",
                        "All controllers stopped, will restart with backoff"
                    );

                    // Reset failure counter if ran stably for long enough
                    if iteration_start.elapsed() >= STABLE_RUN_DURATION {
                        consecutive_failures = 0;
                    }

                    consecutive_failures += 1;
                    if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                        tracing::error!(
                            component = "conf_center_new",
                            mode = "kubernetes",
                            consecutive_failures = consecutive_failures,
                            "Max consecutive failures exceeded, giving up"
                        );
                        return MainFlowExit::Shutdown;
                    }

                    tracing::info!(
                        component = "conf_center_new",
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

    /// Create LeaderElectionConfig from ConfCenterConfig
    fn create_leader_election_config(&self) -> Result<K8sLeaderElectionConfig> {
        let ConfCenterConfig::Kubernetes {
            leader_election: le_config,
            ..
        } = self.config()
        else {
            return Err(anyhow::anyhow!("Not in Kubernetes mode"));
        };

        // Create K8s leader election config
        let config = K8sLeaderElectionConfig::new(&le_config.lease_name, &le_config.lease_namespace)
            .with_lease_duration_secs(le_config.lease_duration_secs)
            .with_renew_period_secs(le_config.renew_period_secs)
            .with_retry_period_secs(le_config.retry_period_secs);

        tracing::info!(
            component = "conf_center_new",
            mode = "kubernetes",
            lease_name = %le_config.lease_name,
            lease_namespace = %le_config.lease_namespace,
            lease_duration_secs = le_config.lease_duration_secs,
            "Leader election configuration loaded"
        );

        Ok(config)
    }

    /// Create K8s controller (new architecture - no ConfigServer dependency)
    fn create_k8s_controller(
        &self,
        client: &Client,
        endpoint_mode: EndpointMode,
    ) -> Result<KubernetesController> {
        let ConfCenterConfig::Kubernetes {
            watch_namespaces,
            label_selector,
            gateway_class,
            metadata_filter,
            ..
        } = self.config()
        else {
            return Err(anyhow::anyhow!("Not in Kubernetes mode"));
        };

        tracing::info!(
            component = "conf_center_new",
            mode = "kubernetes",
            gateway_class = gateway_class,
            namespaces = ?watch_namespaces,
            metadata_filter_enabled = true,
            blocked_annotations_count = metadata_filter.blocked_annotations.len(),
            remove_managed_fields = metadata_filter.remove_managed_fields,
            "Creating Kubernetes controller"
        );

        KubernetesController::with_metadata_filter(
            client.clone(),
            gateway_class.clone(),
            watch_namespaces.clone(),
            label_selector.clone(),
            metadata_filter.clone(),
            endpoint_mode,
        )
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
        // 1. Resolve endpoint mode before creating controller
        let config_endpoint_mode = match self.config() {
            ConfCenterConfig::Kubernetes { endpoint_mode, .. } => *endpoint_mode,
            _ => EndpointMode::EndpointSlice,
        };

        let resolved_mode = resolve_endpoint_mode(client, config_endpoint_mode).await?;

        tracing::info!(
            component = "conf_center_new",
            config_mode = ?config_endpoint_mode,
            resolved_mode = ?resolved_mode,
            "Endpoint mode resolved"
        );

        crate::core::backends::init_global_endpoint_mode(resolved_mode);

        // 2. Create ConfigSyncServer (will be populated when caches are ready)
        let config_sync_server = Arc::new(ConfigSyncServer::new());
        config_sync_server.set_endpoint_mode(resolved_mode);

        // 3. Create Controller with resolved endpoint mode (no ConfigServer dependency)
        let controller = self.create_k8s_controller(client, resolved_mode)?;

        // 4. Spawn controller.run task
        let shutdown_signal = shutdown_handle.signal();
        let tx = event_tx.clone();
        let controller_handle = tokio::spawn(async move {
            let reason = controller.run(shutdown_signal).await.unwrap_or_else(|e| {
                tracing::error!(
                    component = "conf_center_new",
                    mode = "kubernetes",
                    error = %e,
                    "Controller run error"
                );
                ControllerExitReason::AllControllersStopped
            });
            let _ = tx.send(LifecycleEvent::ControllerExit(reason)).await;
        });

        // 5. Spawn caches ready watcher task (monitors PROCESSOR_REGISTRY)
        let css = config_sync_server.clone();
        let tx = event_tx.clone();
        let caches_handle = tokio::spawn(async move {
            const CACHE_READY_TIMEOUT_SECS: u64 = 30;
            let timeout = Duration::from_secs(CACHE_READY_TIMEOUT_SECS);
            let start = Instant::now();

            // Wait for PROCESSOR_REGISTRY to be ready
            while !PROCESSOR_REGISTRY.is_all_ready() && start.elapsed() < timeout {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }

            if PROCESSOR_REGISTRY.is_all_ready() {
                // Register all WatchObjs to ConfigSyncServer
                css.register_all(PROCESSOR_REGISTRY.all_watch_objs());
                let _ = tx.send(LifecycleEvent::CachesReady).await;
            } else {
                let not_ready: Vec<String> = PROCESSOR_REGISTRY
                    .all_kinds()
                    .into_iter()
                    .filter(|kind| {
                        PROCESSOR_REGISTRY
                            .get(kind)
                            .map(|p| !p.is_ready())
                            .unwrap_or(false)
                    })
                    .map(|s| s.to_string())
                    .collect();

                tracing::warn!(
                    component = "conf_center_new",
                    mode = "kubernetes",
                    timeout_secs = CACHE_READY_TIMEOUT_SECS,
                    not_ready = ?not_ready,
                    "Timeout waiting for processors"
                );
                let _ = tx.send(LifecycleEvent::CachesTimeout).await;
            }
        });

        // 6. Spawn leadership loss watcher task
        let lh = leader_handle.clone();
        let tx = event_tx;
        let leader_watcher_handle = tokio::spawn(async move {
            while lh.is_leader() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            let _ = tx.send(LifecycleEvent::LeadershipLost).await;
        });

        Ok((
            WatcherHandles {
                controller: controller_handle,
                caches: caches_handle,
                leader: leader_watcher_handle,
            },
            config_sync_server,
        ))
    }

    /// Calculate exponential backoff duration
    fn backoff(failures: u32) -> Duration {
        Duration::from_secs(1 << failures.min(6))
    }
}
