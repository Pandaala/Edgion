//! Kubernetes mode lifecycle implementation
//!
//! This module manages the complete lifecycle for Kubernetes mode:
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
//!     └── Main Flow (only leader executes)
//!         │
//!         ├── 1. Create ConfigServer
//!         ├── 2. Create KubernetesController
//!         ├── 3. Run Controller (spawns 19 ResourceControllers)
//!         ├── 4. Wait for caches ready OR controller exit
//!         ├── 5. Set config_server = Some (services become available)
//!         ├── 6. Wait for exit signal
//!         │
//!         └── Handle Exit Reason
//!             ├── Shutdown → Exit program
//!             ├── LostLeadership → Back to "Wait for Leadership"
//!             ├── RelinkRequested (410 GONE) → Retry main flow
//!             └── Error → Retry with backoff
//! ```

use super::kubernetes::{
    LeaderElection, LeaderElectionConfig as K8sLeaderElectionConfig, LeaderHandle, ShutdownHandle,
};
use super::{ConfCenter, ConfCenterConfig, ControllerExitReason, KubernetesController};
use crate::core::conf_sync::ConfigServer;
use anyhow::Result;
use kube::Client;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Exit reason from main flow
enum MainFlowExit {
    /// Normal shutdown requested (Ctrl+C, SIGTERM)
    Shutdown,
    /// Lost leadership, need to wait for re-election
    LostLeadership,
}

impl ConfCenter {
    /// K8s mode lifecycle - leader election + main flow with automatic restart
    ///
    /// This is the top-level lifecycle method for Kubernetes mode.
    /// It handles leader election and delegates to main flow when leadership is acquired.
    pub(super) async fn run_k8s_lifecycle(&self) -> Result<()> {
        // 1. Create K8s Client
        let client = Client::try_default().await?;
        tracing::info!(
            component = "conf_center",
            mode = "kubernetes",
            "K8s client initialized"
        );

        // 2. Initialize Leader Election
        let leader_config = self.create_leader_election_config()?;
        let leader_election = LeaderElection::new(client.clone(), leader_config);
        let leader_handle = leader_election.handle();

        // 3. Spawn leader election background task
        let le = leader_election.clone();
        tokio::spawn(async move {
            if let Err(e) = le.run().await {
                tracing::error!(
                    component = "conf_center",
                    error = %e,
                    "Leader election task failed"
                );
            }
        });

        // 4. Create global shutdown handle for signal handling
        let shutdown_handle = ShutdownHandle::new();
        let signal_shutdown = shutdown_handle.clone();
        tokio::spawn(async move {
            signal_shutdown.wait_for_signals().await;
        });

        // 5. Main lifecycle loop
        loop {
            // === Phase 1: Wait for leadership ===
            tracing::info!(
                component = "conf_center",
                mode = "kubernetes",
                "Waiting for leadership..."
            );

            if !leader_handle
                .wait_until_leader_with_shutdown(shutdown_handle.signal())
                .await
            {
                tracing::info!(
                    component = "conf_center",
                    mode = "kubernetes",
                    "Shutdown requested before acquiring leadership"
                );
                return Ok(());
            }

            tracing::info!(
                component = "conf_center",
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
                        component = "conf_center",
                        mode = "kubernetes",
                        "Normal shutdown"
                    );
                    return Ok(());
                }
                MainFlowExit::LostLeadership => {
                    tracing::warn!(
                        component = "conf_center",
                        mode = "kubernetes",
                        "Lost leadership, will wait for re-election"
                    );
                    // Loop back to wait for leadership
                    continue;
                }
            }
        }
    }

    /// Main flow - runs only when this instance is the leader
    ///
    /// This method contains the main control loop that:
    /// 1. Creates ConfigServer
    /// 2. Creates and runs KubernetesController
    /// 3. Handles errors and 410 GONE with automatic retry
    /// 4. Returns when shutdown or leadership is lost
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
                self.set_config_server(None);
                return MainFlowExit::LostLeadership;
            }

            tracing::info!(
                component = "conf_center",
                mode = "kubernetes",
                consecutive_failures = consecutive_failures,
                "Starting main flow iteration"
            );

            let iteration_start = Instant::now();

            // Run one iteration of the main flow
            let result = self
                .run_iteration(client, leader_handle, shutdown_handle)
                .await;

            match result {
                IterationResult::Shutdown => {
                    self.set_config_server(None);
                    return MainFlowExit::Shutdown;
                }
                IterationResult::LostLeadership => {
                    self.set_config_server(None);
                    return MainFlowExit::LostLeadership;
                }
                IterationResult::RelinkRequested(reason) => {
                    // 410 GONE - normal reconnection, don't count as failure
                    self.set_config_server(None);

                    tracing::info!(
                        component = "conf_center",
                        mode = "kubernetes",
                        reason = reason,
                        "Relink requested (410 GONE), restarting immediately"
                    );

                    // Reset failure counter since this is not a real failure
                    consecutive_failures = 0;

                    // Small delay to avoid tight loop
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                IterationResult::Error(reason) => {
                    // Real error - count as failure and apply backoff
                    self.set_config_server(None);

                    tracing::warn!(
                        component = "conf_center",
                        mode = "kubernetes",
                        reason = reason,
                        "Iteration failed, will restart with backoff"
                    );

                    // Reset failure counter if ran stably for long enough
                    if iteration_start.elapsed() >= STABLE_RUN_DURATION {
                        consecutive_failures = 0;
                    }

                    consecutive_failures += 1;
                    if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                        tracing::error!(
                            component = "conf_center",
                            mode = "kubernetes",
                            consecutive_failures = consecutive_failures,
                            "Max consecutive failures exceeded, giving up"
                        );
                        // Return shutdown to exit the program
                        return MainFlowExit::Shutdown;
                    }

                    // Exponential backoff before restart
                    let backoff = Duration::from_secs(1 << consecutive_failures.min(6));
                    tracing::info!(
                        component = "conf_center",
                        mode = "kubernetes",
                        backoff_secs = backoff.as_secs(),
                        consecutive_failures = consecutive_failures,
                        "Waiting before restart"
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    /// Run a single iteration of the main flow
    ///
    /// Steps:
    /// 1. Create ConfigServer
    /// 2. Create KubernetesController
    /// 3. Run controller in background
    /// 4. Wait for caches ready OR controller exit
    /// 5. Set config_server = Some
    /// 6. Wait for exit signal
    async fn run_iteration(
        &self,
        client: &Client,
        leader_handle: &LeaderHandle,
        shutdown_handle: &ShutdownHandle,
    ) -> IterationResult {
        // Step 1: Create ConfigServer
        let config_server = Arc::new(ConfigServer::new(&self.conf_sync_config));

        // Step 2: Create KubernetesController
        let controller = match self.create_k8s_controller(client, &config_server) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(
                    component = "conf_center",
                    mode = "kubernetes",
                    error = %e,
                    "Failed to create K8s controller"
                );
                return IterationResult::Error(format!("controller_creation_failed: {}", e));
            }
        };

        // Step 3: Run controller in background
        let (exit_tx, mut exit_rx) = oneshot::channel::<ControllerExitReason>();
        let shutdown_signal = shutdown_handle.signal();

        let controller_handle = tokio::spawn(async move {
            let exit_reason = match controller.run(shutdown_signal).await {
                Ok(reason) => reason,
                Err(e) => {
                    tracing::error!(
                        component = "conf_center",
                        mode = "kubernetes",
                        error = %e,
                        "Controller run error"
                    );
                    ControllerExitReason::AllControllersStopped
                }
            };
            let _ = exit_tx.send(exit_reason);
        });

        // Step 4: Wait for caches ready OR controller exit (whichever first)
        let mut caches_ready = false;
        let exit_reason = tokio::select! {
            // Check for leadership loss
            _ = wait_for_leadership_loss(leader_handle) => {
                tracing::warn!(
                    component = "conf_center",
                    mode = "kubernetes",
                    "Lost leadership during cache initialization"
                );
                // Abort controller
                controller_handle.abort();
                return IterationResult::LostLeadership;
            }

            // Wait for caches to be ready
            _ = self.wait_caches_ready(&config_server, 30) => {
                caches_ready = true;

                // Step 5: Set config_server = Some (services become available)
                self.set_config_server(Some(config_server));
                tracing::info!(
                    component = "conf_center",
                    mode = "kubernetes",
                    "ConfigServer is ready, gRPC and Admin API can serve requests"
                );

                // Step 6: Wait for exit signal
                tokio::select! {
                    // Check for leadership loss
                    _ = wait_for_leadership_loss(leader_handle) => {
                        tracing::warn!(
                            component = "conf_center",
                            mode = "kubernetes",
                            "Lost leadership during normal operation"
                        );
                        // Abort controller
                        controller_handle.abort();
                        return IterationResult::LostLeadership;
                    }

                    // Wait for controller exit
                    result = &mut exit_rx => {
                        match result {
                            Ok(reason) => reason,
                            Err(_) => {
                                tracing::error!(
                                    component = "conf_center",
                                    mode = "kubernetes",
                                    "Controller task ended unexpectedly (channel closed)"
                                );
                                ControllerExitReason::AllControllersStopped
                            }
                        }
                    }
                }
            }

            // Controller exited before caches ready
            result = &mut exit_rx => {
                tracing::warn!(
                    component = "conf_center",
                    mode = "kubernetes",
                    "Controller exited before caches were ready"
                );
                match result {
                    Ok(reason) => reason,
                    Err(_) => {
                        tracing::error!(
                            component = "conf_center",
                            mode = "kubernetes",
                            "Controller task ended unexpectedly (channel closed)"
                        );
                        ControllerExitReason::AllControllersStopped
                    }
                }
            }
        };

        // Ensure controller task is cleaned up
        let _ = controller_handle.await;

        // Clear config_server if it was set
        if caches_ready {
            self.set_config_server(None);
        }

        // Convert ControllerExitReason to IterationResult
        match exit_reason {
            ControllerExitReason::Shutdown => IterationResult::Shutdown,
            ControllerExitReason::LostLeadership => IterationResult::LostLeadership,
            ControllerExitReason::RelinkRequested(reason) => {
                // 410 GONE is a normal reconnection, not a failure
                IterationResult::RelinkRequested(format!("{:?}", reason))
            }
            ControllerExitReason::AllControllersStopped => {
                // This is an actual error
                IterationResult::Error("all_controllers_stopped".to_string())
            }
        }
    }

    /// Create LeaderElectionConfig from ConfCenterConfig
    fn create_leader_election_config(&self) -> Result<K8sLeaderElectionConfig> {
        let ConfCenterConfig::Kubernetes {
            leader_election: le_config,
            ..
        } = &self.config
        else {
            return Err(anyhow::anyhow!("Not in Kubernetes mode"));
        };

        // Create K8s leader election config
        let config = K8sLeaderElectionConfig::new(&le_config.lease_name, &le_config.lease_namespace)
            .with_lease_duration_secs(le_config.lease_duration_secs)
            .with_renew_period_secs(le_config.renew_period_secs)
            .with_retry_period_secs(le_config.retry_period_secs);

        tracing::info!(
            component = "conf_center",
            mode = "kubernetes",
            lease_name = %le_config.lease_name,
            lease_namespace = %le_config.lease_namespace,
            lease_duration_secs = le_config.lease_duration_secs,
            "Leader election configuration loaded"
        );

        Ok(config)
    }

    /// Create K8s controller
    fn create_k8s_controller(
        &self,
        client: &Client,
        config_server: &Arc<ConfigServer>,
    ) -> Result<KubernetesController> {
        let ConfCenterConfig::Kubernetes {
            watch_namespaces,
            label_selector,
            gateway_class,
            metadata_filter,
            ..
        } = &self.config
        else {
            return Err(anyhow::anyhow!("Not in Kubernetes mode"));
        };

        tracing::info!(
            component = "conf_center",
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
            config_server.clone(),
            gateway_class.clone(),
            watch_namespaces.clone(),
            label_selector.clone(),
            metadata_filter.clone(),
        )
    }
}

/// Result of a single iteration
enum IterationResult {
    /// Normal shutdown requested
    Shutdown,
    /// Lost leadership
    LostLeadership,
    /// 410 GONE - need to restart but don't count as failure
    RelinkRequested(String),
    /// Real error - count as failure and apply backoff
    Error(String),
}

/// Wait until leadership is lost
async fn wait_for_leadership_loss(leader_handle: &LeaderHandle) {
    loop {
        if !leader_handle.is_leader() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
