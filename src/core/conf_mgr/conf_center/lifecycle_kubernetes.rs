//! Kubernetes mode lifecycle implementation
//!
//! This module contains the lifecycle management logic for Kubernetes mode:
//! - Leader election (via KubernetesController)
//! - Watch K8s resources using kube-runtime Controller pattern
//! - Automatic restart with exponential backoff on failure

use super::{ConfCenter, ConfCenterConfig, ControllerExitReason, KubernetesController};
use crate::core::conf_sync::ConfigServer;
use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

impl ConfCenter {
    /// K8s mode lifecycle - clean loop with automatic restart
    ///
    /// Loop:
    /// 1. Create ConfigServer
    /// 2. Create Controller (includes leader election internally)
    /// 3. Run controller in background
    /// 4. Wait for caches ready OR controller exit (whichever first)
    ///    - If caches ready first: set config_server = Some, then wait for exit
    ///    - If controller exits first: skip setting config_server
    /// 5. Set config_server = None
    /// 6. Handle exit reason: shutdown or restart with backoff
    pub(super) async fn run_k8s_lifecycle(&self) -> Result<()> {
        const MAX_CONSECUTIVE_FAILURES: u32 = 10;
        const STABLE_RUN_DURATION: Duration = Duration::from_secs(300); // 5 minutes

        let mut consecutive_failures: u32 = 0;

        loop {
            tracing::info!(
                component = "conf_center",
                mode = "kubernetes",
                consecutive_failures = consecutive_failures,
                "Starting K8s lifecycle iteration"
            );

            let iteration_start = Instant::now();

            // 1. Create ConfigServer
            let config_server = Arc::new(ConfigServer::new(&self.conf_sync_config));

            // 2. Create Controller (includes leader election internally)
            let controller = match self.create_k8s_controller(&config_server).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(
                        component = "conf_center",
                        mode = "kubernetes",
                        error = %e,
                        "Failed to create K8s controller"
                    );
                    consecutive_failures += 1;
                    if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                        return Err(anyhow::anyhow!("Max consecutive failures exceeded: {}", e));
                    }
                    let backoff = Duration::from_secs(1 << consecutive_failures.min(6));
                    tokio::time::sleep(backoff).await;
                    continue;
                }
            };

            // 3. Run controller in background and get exit reason via channel
            let (exit_tx, mut exit_rx) = oneshot::channel::<ControllerExitReason>();
            let controller_handle = tokio::spawn(async move {
                let exit_reason = match controller.run().await {
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
                // Send exit reason (ignore error if receiver dropped)
                let _ = exit_tx.send(exit_reason);
            });

            // 4. Wait for caches ready OR controller exit (whichever comes first)
            // This avoids waiting 30s timeout if controller exits early
            let mut caches_ready = false;
            let exit_reason = tokio::select! {
                _ = self.wait_caches_ready(&config_server, 30) => {
                    caches_ready = true;
                    // Caches ready, set config_server
                    self.set_config_server(Some(config_server));
                    tracing::info!(
                        component = "conf_center",
                        mode = "kubernetes",
                        "ConfigServer is ready, services can process requests"
                    );

                    // Now wait for controller to exit
                    match exit_rx.await {
                        Ok(reason) => reason,
                        Err(_) => {
                            tracing::error!(
                                component = "conf_center",
                                mode = "kubernetes",
                                "Controller task ended unexpectedly"
                            );
                            ControllerExitReason::AllControllersStopped
                        }
                    }
                }
                result = &mut exit_rx => {
                    // Controller exited before caches ready - don't set config_server
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
                                "Controller task ended unexpectedly"
                            );
                            ControllerExitReason::AllControllersStopped
                        }
                    }
                }
            };

            // Ensure controller task is done
            let _ = controller_handle.await;

            // 5. Set config_server = None (only if it was set)
            if caches_ready {
                self.set_config_server(None);
            }

            // 6. Handle exit reason
            match exit_reason {
                ControllerExitReason::Shutdown => {
                    tracing::info!(component = "conf_center", mode = "kubernetes", "Normal shutdown");
                    return Ok(());
                }
                reason => {
                    tracing::warn!(
                        component = "conf_center",
                        mode = "kubernetes",
                        exit_reason = ?reason,
                        "Controller exited, will restart"
                    );

                    // Reset counter if ran stably for long enough
                    if iteration_start.elapsed() >= STABLE_RUN_DURATION {
                        consecutive_failures = 0;
                    }

                    consecutive_failures += 1;
                    if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                        return Err(anyhow::anyhow!("Max consecutive failures exceeded after {:?}", reason));
                    }

                    // Backoff before restart
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

    /// Create K8s controller
    pub(super) async fn create_k8s_controller(
        &self,
        config_server: &Arc<ConfigServer>,
    ) -> Result<KubernetesController> {
        let ConfCenterConfig::Kubernetes {
            watch_namespaces,
            label_selector,
            gateway_class,
            metadata_filter,
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
            "Creating Kubernetes controller with metadata filter"
        );

        KubernetesController::with_metadata_filter(
            config_server.clone(),
            gateway_class.clone(),
            watch_namespaces.clone(),
            label_selector.clone(),
            metadata_filter.clone(),
        )
        .await
    }
}
