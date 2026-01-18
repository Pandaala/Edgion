//! Leader election for Kubernetes controller
//!
//! Uses Kubernetes Lease resources for distributed leader election.
//! Only the leader instance will run reconciliation loops.

use anyhow::Result;
use k8s_openapi::api::coordination::v1::Lease;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;
use k8s_openapi::chrono::Utc;
use kube::api::{Api, Patch, PatchParams, PostParams};
use kube::Client;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::metrics::controller_metrics;

/// Default lease duration in seconds
const DEFAULT_LEASE_DURATION_SECS: i32 = 15;
/// Default renew deadline in seconds
const DEFAULT_RENEW_DEADLINE_SECS: u64 = 10;
/// Default retry period in seconds  
const DEFAULT_RETRY_PERIOD_SECS: u64 = 2;

/// Leader election configuration
#[derive(Clone, Debug)]
pub struct LeaderElectionConfig {
    /// Lease name (usually controller name)
    pub lease_name: String,
    /// Lease namespace
    pub lease_namespace: String,
    /// Identity of this instance (usually pod name)
    pub identity: String,
    /// Lease duration in seconds
    pub lease_duration_secs: i32,
    /// How often to renew the lease
    pub renew_period_secs: u64,
    /// How often non-leaders should retry acquiring the lease
    pub retry_period_secs: u64,
}

impl LeaderElectionConfig {
    /// Create a new leader election config
    ///
    /// Requires either `POD_NAME` or `HOSTNAME` environment variable to be set.
    /// In Kubernetes, this is typically done via the Downward API.
    ///
    /// # Panics
    /// Panics if neither `POD_NAME` nor `HOSTNAME` environment variable is set.
    pub fn new(lease_name: impl Into<String>, lease_namespace: impl Into<String>) -> Self {
        // Try to get pod name from environment
        let identity = std::env::var("POD_NAME").or_else(|_| std::env::var("HOSTNAME")).expect(
            "Leader election requires POD_NAME or HOSTNAME environment variable to be set. \
                     In Kubernetes, use the Downward API to inject the pod name.",
        );

        Self {
            lease_name: lease_name.into(),
            lease_namespace: lease_namespace.into(),
            identity,
            lease_duration_secs: DEFAULT_LEASE_DURATION_SECS,
            renew_period_secs: DEFAULT_RENEW_DEADLINE_SECS,
            retry_period_secs: DEFAULT_RETRY_PERIOD_SECS,
        }
    }

    /// Set the identity for this instance
    pub fn with_identity(mut self, identity: impl Into<String>) -> Self {
        self.identity = identity.into();
        self
    }

    /// Set the lease duration
    pub fn with_lease_duration_secs(mut self, secs: i32) -> Self {
        self.lease_duration_secs = secs;
        self
    }
}

/// Leader election state
#[derive(Clone)]
pub struct LeaderElection {
    client: Client,
    config: LeaderElectionConfig,
    is_leader: Arc<AtomicBool>,
}

impl LeaderElection {
    /// Create a new leader election instance
    pub fn new(client: Client, config: LeaderElectionConfig) -> Self {
        Self {
            client,
            config,
            is_leader: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if this instance is the leader
    pub fn is_leader(&self) -> bool {
        self.is_leader.load(Ordering::Relaxed)
    }

    /// Get a handle that can be cloned and used to check leader status
    pub fn handle(&self) -> LeaderHandle {
        LeaderHandle {
            is_leader: self.is_leader.clone(),
        }
    }

    /// Run the leader election loop
    ///
    /// This should be spawned as a background task. It will continuously
    /// try to acquire or renew the lease.
    pub async fn run(&self) -> Result<()> {
        let lease_api: Api<Lease> = Api::namespaced(self.client.clone(), &self.config.lease_namespace);

        tracing::info!(
            component = "leader_election",
            lease_name = %self.config.lease_name,
            lease_namespace = %self.config.lease_namespace,
            identity = %self.config.identity,
            "Starting leader election"
        );

        // Ensure lease exists
        self.ensure_lease_exists(&lease_api).await?;

        loop {
            // Use different intervals: leader renews more frequently, non-leader retries slower
            let sleep_duration = if self.is_leader.load(Ordering::Relaxed) {
                Duration::from_secs(self.config.renew_period_secs)
            } else {
                Duration::from_secs(self.config.retry_period_secs)
            };
            tokio::time::sleep(sleep_duration).await;

            match self.try_acquire_or_renew(&lease_api).await {
                Ok(true) => {
                    if !self.is_leader.swap(true, Ordering::Relaxed) {
                        tracing::info!(
                            component = "leader_election",
                            identity = %self.config.identity,
                            "Acquired leadership"
                        );
                        controller_metrics().set_leader(true);
                    }
                }
                Ok(false) => {
                    if self.is_leader.swap(false, Ordering::Relaxed) {
                        tracing::info!(
                            component = "leader_election",
                            identity = %self.config.identity,
                            "Lost leadership"
                        );
                        controller_metrics().set_leader(false);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        component = "leader_election",
                        error = %e,
                        "Failed to acquire/renew lease"
                    );
                    // On error, assume we lost leadership to be safe
                    if self.is_leader.swap(false, Ordering::Relaxed) {
                        controller_metrics().set_leader(false);
                    }
                }
            }
        }
    }

    /// Ensure the lease resource exists
    async fn ensure_lease_exists(&self, api: &Api<Lease>) -> Result<()> {
        match api.get(&self.config.lease_name).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(e)) if e.code == 404 => {
                // Create the lease
                let lease = Lease {
                    metadata: kube::api::ObjectMeta {
                        name: Some(self.config.lease_name.clone()),
                        namespace: Some(self.config.lease_namespace.clone()),
                        ..Default::default()
                    },
                    spec: Some(k8s_openapi::api::coordination::v1::LeaseSpec {
                        holder_identity: Some(self.config.identity.clone()),
                        lease_duration_seconds: Some(self.config.lease_duration_secs),
                        acquire_time: Some(MicroTime(Utc::now())),
                        renew_time: Some(MicroTime(Utc::now())),
                        lease_transitions: Some(0),
                        ..Default::default()
                    }),
                };

                match api.create(&PostParams::default(), &lease).await {
                    Ok(_) => {
                        tracing::info!(
                            component = "leader_election",
                            lease_name = %self.config.lease_name,
                            "Created lease resource"
                        );
                        Ok(())
                    }
                    // Handle race condition: another instance created it first
                    Err(kube::Error::Api(create_err)) if create_err.code == 409 => {
                        tracing::debug!(
                            component = "leader_election",
                            lease_name = %self.config.lease_name,
                            "Lease already created by another instance"
                        );
                        Ok(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Try to acquire or renew the lease
    async fn try_acquire_or_renew(&self, api: &Api<Lease>) -> Result<bool> {
        let lease = api.get(&self.config.lease_name).await?;
        let spec = lease.spec.as_ref();

        let current_holder = spec.and_then(|s| s.holder_identity.as_deref()).unwrap_or("");
        let is_current_holder = current_holder == self.config.identity;

        // Check if lease is expired
        let is_expired = if let Some(renew_time) = spec.and_then(|s| s.renew_time.as_ref()) {
            let lease_duration = spec
                .and_then(|s| s.lease_duration_seconds)
                .unwrap_or(DEFAULT_LEASE_DURATION_SECS) as i64;
            let expiry = renew_time.0 + chrono::Duration::seconds(lease_duration);
            Utc::now() > expiry
        } else {
            true // No renew time means expired
        };

        // Can acquire if we're the holder, or the lease is expired
        if !is_current_holder && !is_expired {
            tracing::trace!(
                component = "leader_election",
                current_holder = %current_holder,
                identity = %self.config.identity,
                "Lease held by another instance"
            );
            return Ok(false);
        }

        // Try to update the lease
        let transitions = if is_current_holder {
            spec.and_then(|s| s.lease_transitions).unwrap_or(0)
        } else {
            spec.and_then(|s| s.lease_transitions).unwrap_or(0) + 1
        };

        let patch = serde_json::json!({
            "spec": {
                "holderIdentity": self.config.identity,
                "leaseDurationSeconds": self.config.lease_duration_secs,
                "renewTime": MicroTime(Utc::now()),
                "leaseTransitions": transitions,
            }
        });

        api.patch(
            &self.config.lease_name,
            &PatchParams::apply("edgion-controller").force(),
            &Patch::Apply(&patch),
        )
        .await?;

        Ok(true)
    }
}

/// Handle to check leader status from anywhere
#[derive(Clone)]
pub struct LeaderHandle {
    is_leader: Arc<AtomicBool>,
}

impl LeaderHandle {
    /// Check if this instance is the leader
    pub fn is_leader(&self) -> bool {
        self.is_leader.load(Ordering::Relaxed)
    }

    /// Wait until this instance becomes leader
    ///
    /// This will block indefinitely until leadership is acquired.
    pub async fn wait_until_leader(&self) {
        while !self.is_leader() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait until this instance becomes leader, with cancellation support
    ///
    /// Returns `true` if leadership was acquired, `false` if cancelled via shutdown signal.
    pub async fn wait_until_leader_with_shutdown(
        &self,
        mut shutdown: crate::core::conf_mgr::conf_center::kubernetes::shutdown::ShutdownSignal,
    ) -> bool {
        loop {
            if self.is_leader() {
                return true;
            }

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                _ = shutdown.wait() => {
                    tracing::info!(
                        component = "leader_election",
                        "Shutdown requested while waiting for leadership"
                    );
                    return false;
                }
            }
        }
    }
}
