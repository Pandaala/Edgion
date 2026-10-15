//! Leader election for Kubernetes controller
//!
//! Uses Kubernetes Lease resources for distributed leader election.
//! Only the leader instance will run reconciliation loops.

use anyhow::Result;
use k8s_openapi::api::coordination::v1::Lease;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;
use k8s_openapi::chrono::Utc;
use kube::api::{Api, Patch, PatchParams, PostParams};
use kube::Client;
use parking_lot::RwLock;
use std::future::{pending, Future};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

use super::controller_metrics;

/// Default lease duration in seconds (matches client-go default).
const DEFAULT_LEASE_DURATION_SECS: i32 = 15;
/// Default renew deadline in seconds: leader retains leadership across
/// transient errors as long as the previous successful renew was within
/// this window. Mirrors client-go's `RenewDeadline`.
const DEFAULT_RENEW_DEADLINE_SECS: u64 = 10;
/// Default retry period in seconds: sleep between every renew/acquire
/// attempt. Mirrors client-go's `RetryPeriod`.
const DEFAULT_RETRY_PERIOD_SECS: u64 = 2;
/// Bound on the best-effort lease release during shutdown. The election task
/// is detached (nothing joins it), so a release still in flight when the
/// process exits is silently cut off — this bound does not gate process exit;
/// it keeps the shutdown arm from stalling the pod-label patch behind the kube
/// client's much longer default request timeout. Any failure mode degrades to
/// natural lease expiry, i.e. the pre-release behavior.
const RELEASE_LEASE_TIMEOUT_SECS: u64 = 3;
pub(crate) const LEADER_TERM_ANNOTATION: &str = "edgion.io/leader-term";

/// Stable identity for one successful local leadership acquisition.
///
/// Lease renewals preserve this value. Credential publication validates it
/// against the live Lease immediately before every external side effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaderFence {
    pub lease_uid: String,
    pub holder_identity: String,
    pub term: String,
    pub lease_transitions: i32,
}

/// Result of validating a captured leadership fence against the live Lease.
#[derive(Debug)]
pub enum LeaderFenceValidation {
    /// The exact captured leadership term is still current.
    Current,
    /// The Lease or local leader state authoritatively identifies another term.
    Lost,
    /// The Lease could not be read, so authority cannot currently be proven.
    Uncertain(kube::Error),
}

/// Leader election configuration
#[derive(Clone, Debug)]
pub struct LeaderElectionConfig {
    /// Lease name (usually controller name)
    pub lease_name: String,
    /// Lease namespace
    pub lease_namespace: String,
    /// Current Pod namespace (used for patching pod labels)
    pub pod_namespace: String,
    /// Identity of this instance (usually pod name)
    pub identity: String,
    /// Lease duration in seconds (server-side validity of the Lease).
    pub lease_duration_secs: i32,
    /// Window (since last successful renew) within which transient errors
    /// do not cause leadership loss. Must satisfy
    /// `retry_period_secs < renew_deadline_secs < lease_duration_secs`.
    pub renew_deadline_secs: u64,
    /// Single sleep cadence between every acquire/renew attempt. Mirrors
    /// client-go's `RetryPeriod`. A transient failure window of
    /// `renew_deadline_secs` therefore produces roughly
    /// `renew_deadline_secs / retry_period_secs` retry attempts before
    /// step-down.
    pub retry_period_secs: u64,
}

impl LeaderElectionConfig {
    /// Create a new leader election config
    ///
    /// Requires either `POD_NAME` or `HOSTNAME` environment variable to be set.
    /// In Kubernetes, this is typically done via the Downward API.
    ///
    /// # Errors
    /// Returns error if neither `POD_NAME` nor `HOSTNAME` environment variable is set.
    pub fn new(lease_name: impl Into<String>, lease_namespace: impl Into<String>) -> Result<Self> {
        let lease_namespace = lease_namespace.into();
        let pod_namespace = std::env::var("POD_NAMESPACE").unwrap_or_else(|_| lease_namespace.clone());

        // Try to get pod name from environment
        let identity = std::env::var("POD_NAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .map_err(|_| {
                anyhow::anyhow!(
                    "Leader election requires POD_NAME or HOSTNAME environment variable to be set. \
                     In Kubernetes, use the Downward API to inject the pod name."
                )
            })?;

        Ok(Self {
            lease_name: lease_name.into(),
            lease_namespace,
            pod_namespace,
            identity,
            lease_duration_secs: DEFAULT_LEASE_DURATION_SECS,
            renew_deadline_secs: DEFAULT_RENEW_DEADLINE_SECS,
            retry_period_secs: DEFAULT_RETRY_PERIOD_SECS,
        })
    }

    /// Set the lease duration
    pub fn with_lease_duration_secs(mut self, secs: i32) -> Self {
        self.lease_duration_secs = secs;
        self
    }

    /// Set the renew deadline (transient-error tolerance window).
    pub fn with_renew_deadline_secs(mut self, secs: u64) -> Self {
        self.renew_deadline_secs = secs;
        self
    }

    /// Set the retry period (how often non-leaders retry acquiring the lease)
    pub fn with_retry_period_secs(mut self, secs: u64) -> Self {
        self.retry_period_secs = secs;
        self
    }

    /// Validate timing invariants required by client-go-style leader election:
    /// `retry_period_secs < renew_deadline_secs < lease_duration_secs`, and all
    /// values must be > 0. Violations risk split-brain (deadline ≥ lease) or
    /// defeat the deadline (retry ≥ deadline).
    pub fn validate(&self) -> Result<()> {
        if self.lease_duration_secs <= 0 {
            anyhow::bail!(
                "leader election misconfig: lease_duration_secs must be > 0 (got {})",
                self.lease_duration_secs
            );
        }
        let lease_duration = self.lease_duration_secs as u64;
        if self.retry_period_secs == 0 {
            anyhow::bail!("leader election misconfig: retry_period_secs must be > 0");
        }
        if self.renew_deadline_secs == 0 {
            anyhow::bail!("leader election misconfig: renew_deadline_secs must be > 0");
        }
        if self.renew_deadline_secs >= lease_duration {
            anyhow::bail!(
                "leader election misconfig: renew_deadline_secs ({}) must be < lease_duration_secs ({}); \
                 otherwise the leader can renew after the lease has already expired on the apiserver, \
                 risking split-brain",
                self.renew_deadline_secs,
                lease_duration
            );
        }
        if self.retry_period_secs >= self.renew_deadline_secs {
            anyhow::bail!(
                "leader election misconfig: retry_period_secs ({}) must be < renew_deadline_secs ({}); \
                 retry interval at or above the deadline leaves no room to recover from a transient error",
                self.retry_period_secs,
                self.renew_deadline_secs
            );
        }
        Ok(())
    }
}

/// Leader election state
#[derive(Clone)]
pub struct LeaderElection {
    client: Client,
    config: LeaderElectionConfig,
    /// Current leadership flag, published as a `watch` channel so process-wide
    /// singleton components (the federation client supervisor) can subscribe to
    /// edges instead of polling. The sender is the authoritative storage; there
    /// is no separate atomic. `send_replace` returns the previous value, exactly
    /// like the `AtomicBool::swap` it replaced.
    is_leader: Arc<watch::Sender<bool>>,
    current_fence: Arc<RwLock<Option<LeaderFence>>>,
}

impl LeaderElection {
    /// Create a new leader election instance.
    ///
    /// `is_leader` is supplied by the owning center so the same sender can be
    /// created before `start()` and subscribed to via
    /// `CenterLifeCycle::leader_signal`.
    pub fn new(client: Client, config: LeaderElectionConfig, is_leader: Arc<watch::Sender<bool>>) -> Self {
        Self {
            client,
            config,
            is_leader,
            current_fence: Arc::new(RwLock::new(None)),
        }
    }

    /// Check if this instance is the leader
    pub fn is_leader(&self) -> bool {
        *self.is_leader.borrow()
    }

    /// Get a handle that can be cloned and used to check leader status
    pub fn handle(&self) -> LeaderHandle {
        LeaderHandle {
            is_leader: self.is_leader.clone(),
            client: self.client.clone(),
            lease_name: self.config.lease_name.clone(),
            lease_namespace: self.config.lease_namespace.clone(),
            current_fence: self.current_fence.clone(),
        }
    }

    /// Perform strict startup checks required by leader-based routing.
    ///
    /// This validates:
    /// - Lease resource can be created/read in `lease_namespace`
    /// - Current Pod label can be patched in `pod_namespace`
    pub async fn preflight_check(&self) -> Result<()> {
        // Fail fast on bad timing config before touching the apiserver: bad
        // values here can cause split-brain at runtime, which is much worse
        // than a startup error.
        self.config.validate()?;
        let lease_api: Api<Lease> = Api::namespaced(self.client.clone(), &self.config.lease_namespace);
        self.ensure_lease_exists(&lease_api).await?;
        self.ensure_pod_label_patch_ready().await?;
        Ok(())
    }

    /// Run the leader election loop
    ///
    /// This should be spawned as a background task. It will continuously
    /// try to acquire or renew the lease.
    ///
    /// If `shutdown_signal` is provided, the loop will exit gracefully when
    /// shutdown is requested, allowing other pods to acquire leadership faster.
    pub async fn run(&self, mut shutdown_signal: Option<super::ShutdownSignal>) -> Result<()> {
        let lease_api: Api<Lease> = Api::namespaced(self.client.clone(), &self.config.lease_namespace);

        tracing::info!(
            component = "leader_election",
            lease_name = %self.config.lease_name,
            lease_namespace = %self.config.lease_namespace,
            pod_namespace = %self.config.pod_namespace,
            identity = %self.config.identity,
            "Starting leader election"
        );

        let renew_window = Duration::from_secs(self.config.renew_deadline_secs);
        let retry_period = Duration::from_secs(self.config.retry_period_secs);
        let mut deadline = None;
        let mut next_attempt = Instant::now() + retry_period;
        let mut label_synced = true;

        'election: loop {
            // Labels consume this same interval, never an extra retry period.
            let attempt = async {
                tokio::time::sleep_until(next_attempt).await;
                self.try_acquire_or_renew(&lease_api).await
            };
            match election_wait(attempt, deadline, &mut shutdown_signal).await {
                ElectionWait::Shutdown => break,
                ElectionWait::Expired => {
                    self.withdraw_leadership();
                    deadline = None;
                    label_synced = false;
                    tracing::warn!(
                        component = "leader_election",
                        "Lost leadership: renew deadline exceeded"
                    );
                }
                ElectionWait::Completed(Ok(Some(fence))) => {
                    deadline = Some(Instant::now() + renew_window);
                    *self.current_fence.write() = Some(fence);
                    if !self.is_leader.send_replace(true) {
                        controller_metrics().set_leader(true);
                        label_synced = false;
                        tracing::info!(component = "leader_election", identity = %self.config.identity, "Acquired leadership");
                    }
                }
                ElectionWait::Completed(Ok(None)) => {
                    label_synced &= !self.withdraw_leadership();
                    deadline = None;
                }
                ElectionWait::Completed(Err(error)) => {
                    let missing = error
                        .downcast_ref::<kube::Error>()
                        .is_some_and(|error| matches!(error, kube::Error::Api(response) if response.code == 404));
                    if missing {
                        label_synced &= !self.withdraw_leadership();
                        deadline = None;
                        tracing::warn!(
                            component = "leader_election",
                            "Lease missing; withdrawing leadership and recreating lease"
                        );
                        match election_wait(self.ensure_lease_exists(&lease_api), None, &mut shutdown_signal).await {
                            ElectionWait::Shutdown => break,
                            ElectionWait::Completed(Err(error)) => {
                                tracing::warn!(component = "leader_election", error = %error, "Failed to recreate lease; will retry on next tick");
                            }
                            _ => {}
                        }
                    } else {
                        // The active deadline remains armed across this error,
                        // the label opportunity, and the next sleep/Lease request.
                        tracing::warn!(component = "leader_election", error = %error,
                            retaining_leadership = self.is_leader(), "Failed to acquire/renew lease; retrying within active deadline");
                    }
                }
            }
            next_attempt = Instant::now() + retry_period;
            while !label_synced {
                let patch = tokio::time::timeout_at(next_attempt, self.update_pod_leader_label(self.is_leader()));
                match election_wait(patch, deadline, &mut shutdown_signal).await {
                    ElectionWait::Shutdown => break 'election,
                    ElectionWait::Expired => {
                        self.withdraw_leadership();
                        deadline = None;
                        // Pace reacquisition from withdrawal, even if the old
                        // retry timestamp is already due. Try the false label
                        // only after both local authority surfaces are cleared.
                        next_attempt = Instant::now() + retry_period;
                        tracing::warn!(
                            component = "leader_election",
                            "Lost leadership: renew deadline exceeded"
                        );
                    }
                    ElectionWait::Completed(result) => {
                        label_synced = result.unwrap_or(false);
                        break;
                    }
                }
            }
        }

        tracing::info!(component = "leader_election", identity = %self.config.identity,
            "Shutdown signal received, stopping leader election");
        // Withdrawal precedes every shutdown IO, including a hung release.
        if self.withdraw_leadership() {
            match tokio::time::timeout(
                Duration::from_secs(RELEASE_LEASE_TIMEOUT_SECS),
                self.release_lease(&lease_api),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(component = "leader_election", error = %error,
                    "Failed to release lease on shutdown; successor takeover falls back to lease expiry"),
                Err(_) => tracing::warn!(
                    component = "leader_election",
                    "Timed out releasing lease on shutdown; successor takeover falls back to lease expiry"
                ),
            }
            let _ = tokio::time::timeout(retry_period, self.update_pod_leader_label(false)).await;
        }
        Ok(())
    }

    /// Withdraw both local authority surfaces before any best-effort IO.
    fn withdraw_leadership(&self) -> bool {
        let was_leader = self.is_leader.send_replace(false);
        *self.current_fence.write() = None;
        if was_leader {
            tracing::info!(component = "leader_election", identity = %self.config.identity, "Lost leadership");
            controller_metrics().set_leader(false);
        }
        was_leader
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

    /// Patch the current Pod's `edgion.io/leader` label.
    /// Returns whether the patch succeeded. Does not retry internally: the run
    /// loop tracks the result via `label_synced` and re-patches with the
    /// current leadership state on subsequent ticks until it converges.
    async fn update_pod_leader_label(&self, is_leader: bool) -> bool {
        match self.try_patch_pod_leader_label(is_leader).await {
            Ok(_) => {
                tracing::info!(
                    component = "leader_election",
                    identity = %self.config.identity,
                    pod_namespace = %self.config.pod_namespace,
                    is_leader = is_leader,
                    "Updated pod leader label"
                );
                true
            }
            Err(e) => {
                tracing::error!(
                    component = "leader_election",
                    identity = %self.config.identity,
                    pod_namespace = %self.config.pod_namespace,
                    is_leader = is_leader,
                    error = %e,
                    "Failed to update pod leader label; will retry on next election tick"
                );
                false
            }
        }
    }

    /// Strict startup validation for pod label patch capability.
    async fn ensure_pod_label_patch_ready(&self) -> Result<()> {
        self.try_patch_pod_leader_label(false).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed startup pod label permission check: cannot patch pod '{}' in namespace '{}': {}",
                self.config.identity,
                self.config.pod_namespace,
                e
            )
        })
    }

    async fn try_patch_pod_leader_label(&self, is_leader: bool) -> Result<()> {
        let pod_api: Api<Pod> = Api::namespaced(self.client.clone(), &self.config.pod_namespace);
        let label_value = if is_leader { "true" } else { "false" };
        let patch = serde_json::json!({
            "metadata": {
                "labels": {
                    "edgion.io/leader": label_value
                }
            }
        });
        pod_api
            .patch(&self.config.identity, &PatchParams::default(), &Patch::Merge(&patch))
            .await?;
        Ok(())
    }

    /// Try to acquire or renew the lease
    ///
    /// Uses optimistic concurrency control: the `replace()` call carries the
    /// `resourceVersion` from the prior `get()`, so if another replica writes
    /// the Lease in between, the API server returns 409 Conflict and we
    /// return `Ok(None)` (lost the race). This prevents split-brain at the
    /// lease-expiry edge where two replicas could otherwise both observe
    /// `is_expired=true` and both succeed in writing.
    async fn try_acquire_or_renew(&self, api: &Api<Lease>) -> Result<Option<LeaderFence>> {
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
            return Ok(None);
        }

        let now = Utc::now();
        let transitions = if is_current_holder {
            spec.and_then(|s| s.lease_transitions).unwrap_or(0)
        } else {
            spec.and_then(|s| s.lease_transitions).unwrap_or(0) + 1
        };
        let acquire_time = if is_current_holder {
            spec.and_then(|s| s.acquire_time.clone())
        } else {
            // Takeover: reset acquire_time to now per K8s leader-election convention
            Some(MicroTime(now))
        };

        // Clone the full object so metadata.resource_version is preserved as a
        // CAS precondition for replace(). Note: this Lease is exclusively
        // managed by Edgion's leader election; PUT replaces the full object,
        // so any future LeaseSpec fields (e.g., preferred_holder/strategy used
        // by K8s coordinated leader election) would be wiped if a third party
        // ever wrote them. Switch to SSA with explicit field ownership if that
        // changes.
        let new_local_acquisition = !self.is_leader();
        let term = if new_local_acquisition {
            uuid::Uuid::new_v4().to_string()
        } else {
            lease
                .metadata
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get(LEADER_TERM_ANNOTATION))
                .cloned()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        };
        let mut updated = lease.clone();
        updated
            .metadata
            .annotations
            .get_or_insert_with(Default::default)
            .insert(LEADER_TERM_ANNOTATION.to_string(), term.clone());
        updated.spec = Some(k8s_openapi::api::coordination::v1::LeaseSpec {
            holder_identity: Some(self.config.identity.clone()),
            lease_duration_seconds: Some(self.config.lease_duration_secs),
            renew_time: Some(MicroTime(now)),
            lease_transitions: Some(transitions),
            acquire_time,
            ..Default::default()
        });

        match api
            .replace(&self.config.lease_name, &PostParams::default(), &updated)
            .await
        {
            Ok(applied) => {
                let lease_uid = applied.metadata.uid.unwrap_or_default();
                if lease_uid.is_empty() {
                    return Ok(None);
                }
                Ok(Some(LeaderFence {
                    lease_uid,
                    holder_identity: self.config.identity.clone(),
                    term,
                    lease_transitions: transitions,
                }))
            }
            // 409 Conflict: another writer modified the Lease between our get
            // and replace. Lost the race — return Ok(None) so the run loop
            // steps down and re-reads on the next tick. (We deliberately do
            // not "debounce" by re-fetching and checking ownership here: that
            // could re-introduce a brief split-brain window if the lease
            // expires between this tick and the next renew.)
            Err(kube::Error::Api(e)) if e.code == 409 => {
                tracing::debug!(
                    component = "leader_election",
                    identity = %self.config.identity,
                    "Lost lease race (409 Conflict); will retry"
                );
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Release the lease held by this instance (client-go `ReleaseOnCancel`
    /// equivalent): clear `holder_identity` and `renew_time` so the next
    /// `try_acquire_or_renew` on any replica sees the lease as expired
    /// ("no renew time means expired") and acquires it on its next tick,
    /// instead of waiting out `lease_duration_secs`. Clearing the holder
    /// alone would NOT unblock a successor — the acquire path only checks
    /// `is_current_holder || is_expired`, never holder vacancy.
    ///
    /// Only writes if this instance is still the recorded holder; the
    /// `resourceVersion` CAS on `replace()` protects against racing a
    /// successor that acquired in between.
    async fn release_lease(&self, api: &Api<Lease>) -> Result<()> {
        let lease = api.get(&self.config.lease_name).await?;
        let spec = lease.spec.as_ref();

        let current_holder = spec.and_then(|s| s.holder_identity.as_deref()).unwrap_or("");
        if current_holder != self.config.identity {
            // Another replica already holds (or the lease is vacant) —
            // nothing to release.
            return Ok(());
        }

        let transitions = spec.and_then(|s| s.lease_transitions).unwrap_or(0);
        let mut updated = lease.clone();
        updated.spec = Some(k8s_openapi::api::coordination::v1::LeaseSpec {
            holder_identity: None,
            lease_duration_seconds: Some(self.config.lease_duration_secs),
            acquire_time: None,
            renew_time: None,
            lease_transitions: Some(transitions),
            ..Default::default()
        });

        match api
            .replace(&self.config.lease_name, &PostParams::default(), &updated)
            .await
        {
            Ok(_) => {
                tracing::info!(
                    component = "leader_election",
                    identity = %self.config.identity,
                    lease_name = %self.config.lease_name,
                    "Released leader lease on shutdown"
                );
                Ok(())
            }
            // 409 Conflict: another writer modified the Lease between our get
            // and replace. Usually a successor acquiring (release goal met);
            // in the worst case a delayed renew of our own landed server-side,
            // which degrades to natural expiry — the pre-release behavior.
            // Either way there is nothing worth retrying on an exit path.
            Err(kube::Error::Api(e)) if e.code == 409 => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// One election owner races each awaited phase against its unchanged deadline.
enum ElectionWait<T> {
    Completed(T),
    Expired,
    Shutdown,
}

async fn election_wait<T>(
    operation: impl Future<Output = T>,
    deadline: Option<Instant>,
    shutdown: &mut Option<super::ShutdownSignal>,
) -> ElectionWait<T> {
    let expiry = async {
        match deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => pending().await,
        }
    };
    let stopped = async {
        match shutdown {
            Some(signal) => signal.wait().await,
            None => pending().await,
        }
    };
    tokio::select! {
        biased;
        _ = stopped => ElectionWait::Shutdown,
        _ = expiry => ElectionWait::Expired,
        result = operation => {
            // The operation may become ready in a poll that crosses expiry.
            // In particular, it must not publish a returned candidate fence.
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                ElectionWait::Expired
            } else {
                ElectionWait::Completed(result)
            }
        }
    }
}

/// Handle to check leader status from anywhere
#[derive(Clone)]
pub struct LeaderHandle {
    is_leader: Arc<watch::Sender<bool>>,
    client: Client,
    lease_name: String,
    lease_namespace: String,
    current_fence: Arc<RwLock<Option<LeaderFence>>>,
}

impl LeaderHandle {
    /// Check if this instance is the leader
    pub fn is_leader(&self) -> bool {
        *self.is_leader.borrow()
    }

    pub fn current_fence(&self) -> Option<LeaderFence> {
        if !self.is_leader() {
            return None;
        }
        self.current_fence.read().clone()
    }

    /// Re-read the Lease and require an exact current leadership fence.
    pub async fn validate_fence(&self, expected: &LeaderFence) -> bool {
        matches!(
            self.validate_fence_typed(expected).await,
            LeaderFenceValidation::Current
        )
    }

    /// Re-read the Lease and classify a captured leadership fence without
    /// conflating a control-plane read failure with an observed handoff.
    pub async fn validate_fence_typed(&self, expected: &LeaderFence) -> LeaderFenceValidation {
        if !self.is_leader() || self.current_fence().as_ref() != Some(expected) {
            return LeaderFenceValidation::Lost;
        }
        let api: Api<Lease> = Api::namespaced(self.client.clone(), &self.lease_namespace);
        let lease = match api.get(&self.lease_name).await {
            Ok(lease) => lease,
            Err(kube::Error::Api(response)) if response.code == 404 => return LeaderFenceValidation::Lost,
            Err(error) => return LeaderFenceValidation::Uncertain(error),
        };
        let spec = lease.spec.as_ref();
        let matches_expected = lease.metadata.uid.as_deref() == Some(expected.lease_uid.as_str())
            && spec.and_then(|value| value.holder_identity.as_deref()) == Some(expected.holder_identity.as_str())
            && spec.and_then(|value| value.lease_transitions) == Some(expected.lease_transitions)
            && lease
                .metadata
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get(LEADER_TERM_ANNOTATION))
                .is_some_and(|term| term == &expected.term);
        if !matches_expected || !self.is_leader() || self.current_fence().as_ref() != Some(expected) {
            LeaderFenceValidation::Lost
        } else {
            LeaderFenceValidation::Current
        }
    }

    /// Wait until this instance becomes leader, with cancellation support
    ///
    /// Returns `true` if leadership was acquired, `false` if cancelled via shutdown signal.
    pub async fn wait_until_leader_with_shutdown(&self, mut shutdown: super::ShutdownSignal) -> bool {
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

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/leader_election_test/fence_tests.rs"]
mod fence_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/leader_election_test/renewal_tests.rs"]
mod renewal_tests;
