//! ACME Service - Background orchestrator for certificate issuance and renewal
//!
//! The AcmeService is the central coordinator that:
//! 1. Receives notifications from EdgionAcmeHandler when resources change
//! 2. Drives the full ACME flow (account → order → challenge → certificate)
//! 3. Creates/updates K8s Secrets with issued certificates
//! 4. Creates/updates EdgionTls resources for Gateway TLS termination
//! 5. Publishes HTTP-01 challenge tokens on `status.activeChallenges`
//!    (propagated to Gateway via gRPC)
//! 6. Schedules periodic renewal checks
//!
//! ## Design
//!
//! - Global singleton via `Mutex<Option<...>>` + `mpsc::channel` (re-initializable)
//! - In-memory tracking of in-flight operations (prevents duplicate processing)
//! - K8s CRD as the source of truth (status, account); active challenges live
//!   on `status.activeChallenges` written via the `/status` subresource
//! - Account credentials persisted in a dedicated K8s Secret

use futures::stream::{FuturesUnordered, StreamExt};
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::{Api, Patch, PatchParams};
use kube::core::{ApiResource, DynamicObject};
use kube::Client;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use zeroize::Zeroizing;

use super::acme_client::{AcmeCertificateResult, AcmeClient, AcmeOperationGuard};
use super::secret_io::{
    account_credentials_from_json, create_account_secret, extract_account_json_string,
    patch_account_key_into_existing_secret,
};
use crate::conf_mgr::sync_runtime::resource_processor::handlers::route_utils::{
    resolve_acme_listener_attachments, AcmeGatewayRefError,
};
use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::acme_is_owned;
use crate::conf_mgr::{ControllerRuntime, LeaderFence, LeaderFenceValidation, LeaderHandle};
use edgion_common::time::now_rfc3339;
use edgion_resources::duration::parse_duration;
use edgion_resources::resources::edgion_acme::{
    AcmeCertPhase, AcmeChallenge, AcmeSchedulerStatus, ActiveHttpChallenge, EdgionAcme, EdgionAcmeStatus,
    MAX_ACME_RESOLVED_ATTACHMENTS,
};
use edgion_resources::resources::edgion_tls::{EdgionTls, ResolvedListenerAttachment};

// ============================================================================
// Global singleton (re-initializable for controller re-election)
// ============================================================================

static ACME_SERVICE: Mutex<Option<AcmeServiceHandle>> = Mutex::new(None);

/// Monotonic generation counter. Each `start_acme_service()` allocates a fresh
/// generation so that detached timers and the startup scan spawned by an older
/// generation can detect a service restart and no-op instead of injecting stale
/// work into (or holding resources for) the new generation. Starts at 0; the
/// first running generation is 1, so a value of 0 never identifies a live timer.
static ACME_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Test-only serialization lock for the process-global `ACME_SERVICE`.
///
/// `test_timer_generation_isolation` installs a real handle into `ACME_SERVICE`
/// and asserts nothing else enqueues into its receiver within a time window.
/// Any concurrently running test that drives `EdgionAcmeHandler::after_publish` reaches
/// `notify_resource_changed` -> `get_sender()`, which clones whatever handle is
/// currently installed and sends into it — landing a stray message in the
/// isolation test's receiver and tripping its assertion. Every test that touches
/// the global (installs a handle, or reads it via the notify path) must hold this
/// lock so they run serially. Held across `.await`, hence `tokio::sync::Mutex`.
#[cfg(test)]
pub(crate) static ACME_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Handle to send commands to the ACME service
struct AcmeServiceHandle {
    tx: mpsc::Sender<AcmeCommand>,
    /// Generation that owns this handle (see `ACME_GENERATION`).
    generation: u64,
    cancel: CancellationToken,
}

/// Commands that can be sent to the ACME service
#[derive(Debug)]
enum AcmeCommand {
    /// An EdgionAcme resource was created or updated
    ResourceChanged { key: String },
    /// An operator explicitly requested a retry and retry-budget reset.
    AdminTrigger { key: String },
    /// Startup recovery must not reset the persisted retry budget.
    StartupCheck { key: String },
}

/// Captures real notification sends without starting a worker or performing IO.
/// Callers must hold `ACME_TEST_MUTEX` for the capture's entire lifetime.
#[cfg(test)]
pub(crate) struct NotificationCapture {
    rx: mpsc::Receiver<AcmeCommand>,
    generation: u64,
}

#[cfg(test)]
impl NotificationCapture {
    pub(crate) fn install() -> Self {
        let (tx, rx) = mpsc::channel(64);
        let generation = ACME_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let mut service = ACME_SERVICE.lock();
        assert!(service.is_none(), "notification capture requires an idle service");
        *service = Some(AcmeServiceHandle {
            tx,
            generation,
            cancel: CancellationToken::new(),
        });
        Self { rx, generation }
    }

    pub(crate) fn drain(&mut self) -> Vec<String> {
        let mut keys = Vec::new();
        while let Ok(command) = self.rx.try_recv() {
            match command {
                AcmeCommand::ResourceChanged { key } => keys.push(key),
                command => panic!("unexpected ACME command: {command:?}"),
            }
        }
        keys
    }
}

#[cfg(test)]
impl Drop for NotificationCapture {
    fn drop(&mut self) {
        let mut service = ACME_SERVICE.lock();
        if service
            .as_ref()
            .is_some_and(|handle| handle.generation == self.generation)
        {
            service.take();
        }
    }
}

/// Initialize and start the global ACME service.
///
/// Called during controller startup (after caches are ready).
/// Must call `stop_acme_service()` before calling this again (e.g. on re-election).
///
/// On startup, the service scans EdgionAcme resources in the configured watch
/// namespaces to recover renewal timers that were lost during controller restart.
pub fn start_acme_service(
    client: Client,
    watch_namespaces: Vec<String>,
    runtime: Arc<ControllerRuntime>,
    leader: LeaderHandle,
) {
    let mut guard = ACME_SERVICE.lock();
    if guard.is_some() {
        tracing::warn!(
            component = "acme_service",
            "ACME service already running, call stop_acme_service() first"
        );
        return;
    }

    // Allocate a fresh generation for this service instance so the startup
    // scan can recognize a later restart and no-op.
    let generation = ACME_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    let (tx, rx) = mpsc::channel::<AcmeCommand>(256);
    let cancel = CancellationToken::new();
    *guard = Some(AcmeServiceHandle {
        tx,
        generation,
        cancel: cancel.clone(),
    });
    drop(guard); // Release lock before spawning tasks

    let startup_cancel = cancel.clone();
    let service = AcmeServiceWorker::new(client.clone(), runtime, leader, generation, cancel);
    let worker_handle = tokio::spawn(async move {
        service.run(rx).await;
    });
    crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("acme").set(1.0);
    crate::services::supervision::observe_worker("acme", worker_handle, move || {
        // Lock + generation check + gauge write are one critical section: a
        // check-then-write outside the lock could interleave with a full
        // stop+start and zero the NEW generation's series.
        let guard = ACME_SERVICE.lock();
        let current = guard.as_ref().map(|h| h.generation) == Some(generation);
        if current {
            crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("acme").set(0.0);
        }
        current
    });

    // Spawn startup scan: re-schedule renewal checks for all existing EdgionAcme resources.
    // This recovers timers lost during controller restart.
    let startup_client = client;
    tokio::spawn(async move {
        // Wait a bit for processors to finish initial sync.
        if startup_cancel
            .run_until_cancelled(tokio::time::sleep(Duration::from_secs(10)))
            .await
            .is_none()
        {
            return;
        }
        // Skip if this generation's service has already been stopped/replaced,
        // so a stale startup scan cannot enqueue into a later generation.
        if current_generation() != generation {
            tracing::debug!(
                component = "acme_service",
                generation = generation,
                "Skipping startup scan: ACME service generation changed"
            );
            return;
        }
        while current_generation() == generation && !startup_cancel.is_cancelled() {
            if scan_all_acme_resources(startup_client.clone(), &watch_namespaces, generation).await {
                break;
            }
            if startup_cancel
                .run_until_cancelled(tokio::time::sleep(CONTROL_PLANE_RECHECK_DELAY))
                .await
                .is_none()
            {
                break;
            }
        }
    });

    tracing::info!(
        component = "acme_service",
        generation = generation,
        "ACME service started"
    );
}

/// Stop the global ACME service.
///
/// Called alongside `ControllerRuntime::reset()` when the controller
/// shuts down or loses leadership. Dropping the `Sender` causes the worker's
/// `recv()` to return `None`, making it exit its event loop cleanly.
///
/// The service cancellation token stops the actor, its jobs, and the delayed
/// startup scan. The generation guard additionally prevents stale startup work
/// from enqueueing into a later service instance.
///
/// After this call, `start_acme_service()` can be called again on re-election.
pub fn stop_acme_service() {
    let mut guard = ACME_SERVICE.lock();
    if let Some(handle) = guard.take() {
        handle.cancel.cancel();
        crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("acme").set(0.0);
        tracing::info!(component = "acme_service", "ACME service stopped");
    }
    // Dropping the AcmeServiceHandle drops the mpsc::Sender, which closes
    // the channel. The worker loop will observe `recv() => None` and exit.
}

/// Startup scan: enumerate EdgionAcme resources in the configured namespaces and
/// trigger a `ResourceChanged` event for each one.
///
/// This ensures that:
/// - `Ready` certs get their renewal timers re-established
/// - `Pending`/`Failed` certs get retried
/// - Nothing is silently forgotten after a controller restart
async fn scan_all_acme_resources(client: Client, watch_namespaces: &[String], generation: u64) -> bool {
    use kube::api::ListParams;

    tracing::info!(component = "acme_service", "Starting full scan of EdgionAcme resources");

    let result = list_acme_resources(client, watch_namespaces, &ListParams::default()).await;

    match result {
        Ok(items) => {
            let count = items.len();
            for acme in items {
                let ns = acme.metadata.namespace.as_deref().unwrap_or("default");
                let name = acme.metadata.name.as_deref().unwrap_or_default();
                let key = format!("{}/{}", ns, name);
                if let Some(tx) = get_sender_for_generation(generation) {
                    let _ = tx.send(AcmeCommand::StartupCheck { key }).await;
                }
            }
            tracing::info!(
                component = "acme_service",
                count = count,
                "Startup scan complete, triggered check for all EdgionAcme resources"
            );
            true
        }
        Err(e) => {
            tracing::error!(
                component = "acme_service",
                error = %e,
                "Failed to list EdgionAcme resources during startup scan"
            );
            false
        }
    }
}

async fn list_acme_resources(
    client: Client,
    watch_namespaces: &[String],
    params: &kube::api::ListParams,
) -> std::result::Result<Vec<EdgionAcme>, kube::Error> {
    if watch_namespaces.is_empty() {
        return Ok(Api::<EdgionAcme>::all(client).list(params).await?.items);
    }

    let mut items = Vec::new();
    for namespace in watch_namespaces {
        items.extend(
            Api::<EdgionAcme>::namespaced(client.clone(), namespace)
                .list(params)
                .await?
                .items,
        );
    }
    Ok(items)
}

/// Clone the current sender from the global handle.
/// Returns `None` if the ACME service has not been started.
fn get_sender() -> Option<mpsc::Sender<AcmeCommand>> {
    let guard = ACME_SERVICE.lock();
    guard.as_ref().map(|h| h.tx.clone())
}

/// Clone the current sender only if the running service still belongs to
/// `generation`. Detached timers capture the generation that scheduled them and
/// use this so a timer left over from an older service instance cannot enqueue
/// into a newer one after a stop/start (leadership flap, shutdown, re-election).
fn get_sender_for_generation(generation: u64) -> Option<mpsc::Sender<AcmeCommand>> {
    let guard = ACME_SERVICE.lock();
    guard
        .as_ref()
        .filter(|h| h.generation == generation)
        .map(|h| h.tx.clone())
}

/// Generation of the currently running service, or 0 when none is running.
fn current_generation() -> u64 {
    ACME_SERVICE.lock().as_ref().map(|h| h.generation).unwrap_or(0)
}

/// Notify the ACME service that a resource has changed.
///
/// Called from `EdgionAcmeHandler::after_publish()`, from the startup
/// scan, and from the Admin API trigger endpoint.
///
/// Uses async `send().await` to apply back-pressure instead of dropping the
/// event when the bounded channel is full — mirroring `Workqueue::enqueue`.
/// A dropped `ResourceChanged` would mean the resource is never re-checked
/// (no issuance, no renewal-timer re-establishment) until the next event,
/// silently risking certificate expiry. `get_sender()` releases the global
/// lock before this await (it returns a cloned `Sender`), so no sync lock is
/// held across the suspension point. The worker drains fast (one K8s `get`
/// plus a `tokio::spawn` per command), so awaiting here cannot stall the
/// caller for long.
///
/// Returns `true` if the command was enqueued, `false` if the ACME service is
/// not running (no sender) or the channel was closed.
pub async fn notify_resource_changed(key: String) -> bool {
    let Some(tx) = get_sender() else {
        return false;
    };
    match tx.send(AcmeCommand::ResourceChanged { key: key.clone() }).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                component = "acme_service",
                key = %key,
                error = %e,
                "Failed to notify ACME service (channel closed)"
            );
            false
        }
    }
}

/// Explicitly request an ACME retry and reset the retry budget for the current
/// resource generation.
pub async fn trigger_resource(key: String) -> bool {
    let Some(tx) = get_sender() else {
        return false;
    };
    match tx.send(AcmeCommand::AdminTrigger { key: key.clone() }).await {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(
                component = "acme_service",
                key = %key,
                error = %error,
                "Failed to trigger ACME service (channel closed)"
            );
            false
        }
    }
}

// ============================================================================
// Retry policy constants
// ============================================================================

/// Maximum number of consecutive retry attempts before giving up.
/// After reaching this limit, the resource stays in `Failed` state until
/// the user edits the CRD (which resets the retry counter).
const MAX_RETRY_ATTEMPTS: u32 = 5;

/// Short convergence delay for raw-object and processed-cache publication skew.
const ELIGIBILITY_RECHECK_DELAY: Duration = Duration::from_secs(1);

/// Retry delay for Kubernetes/Lease reads whose outcome is unknown.
const CONTROL_PLANE_RECHECK_DELAY: Duration = Duration::from_secs(5);

/// Compute exponential backoff duration for a given attempt.
///
/// Formula: `base * 4^attempt`
///
/// Example with base=300s (MAX_RETRY_ATTEMPTS=5):
///   attempt 0 → 300s   (5min)
///   attempt 1 → 1200s  (20min)
///   attempt 2 → 4800s  (80min)
///   attempt 3 → 19200s (5.3h)
///   attempt 4 → 76800s (21.3h)  ← last retry, then give up
fn exponential_backoff(base_secs: u64, attempt: u32) -> Duration {
    let multiplier = 4u64.saturating_pow(attempt);
    let backoff = base_secs.saturating_mul(multiplier);
    Duration::from_secs(backoff)
}

/// Parse a GEP-2257 duration field from the CR spec, falling back to the
/// documented default when the string is invalid. Handler validation rejects
/// invalid durations at admission; this guards the lenient-admission path
/// where an unvalidated CR can still reach the service.
fn spec_duration_or(value: &str, field: &'static str, default: Duration) -> Duration {
    parse_duration(value).unwrap_or_else(|e| {
        tracing::warn!(
            component = "acme_service",
            field = field,
            value = %value,
            error = %e,
            "Invalid GEP-2257 duration in EdgionAcme spec; using default"
        );
        default
    })
}

// ============================================================================
// ACME Service Worker
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerKind {
    Resource,
    Admin,
    Startup,
    Renewal,
    Retry,
    Eligibility,
}

impl TriggerKind {
    fn resets_retry_budget(self) -> bool {
        matches!(self, Self::Admin)
    }
}

#[derive(Default)]
struct ResourceSchedule {
    busy: bool,
    active_cancel: Option<CancellationToken>,
    active_attempt_id: Option<String>,
    certificate_issued: Option<Arc<AtomicBool>>,
    pending: Option<TriggerKind>,
    deadline: Option<(Instant, TriggerKind)>,
}

impl ResourceSchedule {
    fn merge_pending(&mut self, trigger: TriggerKind) {
        if trigger == TriggerKind::Admin || self.pending.is_none() {
            self.pending = Some(trigger);
        }
    }

    fn set_deadline(&mut self, delay: Duration, trigger: TriggerKind) {
        let candidate = Instant::now() + delay;
        if self
            .deadline
            .is_none_or(|(current, _)| candidate < current || trigger == TriggerKind::Admin)
        {
            self.deadline = Some((candidate, trigger));
        }
    }
}

struct AttemptPlan {
    attempt: AcmeAttempt,
    is_renewal: bool,
    fail_backoff_base: u64,
    attempts_used: u32,
}

enum CheckOutcome {
    Deleted,
    Idle,
    Recheck(Duration, TriggerKind),
    Wait(Duration, TriggerKind),
    Start(Box<AttemptPlan>),
}

enum AttemptOutcome {
    Success(Duration),
    Failed(Option<Duration>),
    Retired,
}

enum WorkerJobResult {
    Checked {
        key: String,
        outcome: CheckOutcome,
    },
    AttemptFinished {
        key: String,
        attempt_id: String,
        outcome: AttemptOutcome,
    },
}

type WorkerJob = Pin<Box<dyn Future<Output = WorkerJobResult> + Send>>;

/// The background scheduler. It alone owns per-resource scheduling state;
/// Kubernetes and CA work runs in `jobs` and reports typed completions back.
struct AcmeServiceWorker {
    client: Client,
    runtime: Arc<ControllerRuntime>,
    leader: Option<LeaderHandle>,
    generation: u64,
    cancel: CancellationToken,
    schedules: HashMap<String, ResourceSchedule>,
    jobs: FuturesUnordered<WorkerJob>,
}

impl AcmeServiceWorker {
    fn new(
        client: Client,
        runtime: Arc<ControllerRuntime>,
        leader: LeaderHandle,
        generation: u64,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            client,
            runtime,
            leader: Some(leader),
            generation,
            cancel,
            schedules: HashMap::new(),
            jobs: FuturesUnordered::new(),
        }
    }

    async fn run(mut self, mut rx: mpsc::Receiver<AcmeCommand>) {
        tracing::info!(component = "acme_service", "Worker started, waiting for commands");

        loop {
            let deadline = self
                .schedules
                .values()
                .filter_map(|state| state.deadline.map(|(deadline, _)| deadline))
                .min()
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(365 * 24 * 3600));
            tokio::select! {
                _ = self.cancel.cancelled() => break,
                command = rx.recv() => {
                    let Some(command) = command else { break };
                    let (key, trigger) = match command {
                        AcmeCommand::ResourceChanged { key } => (key, TriggerKind::Resource),
                        AcmeCommand::AdminTrigger { key } => (key, TriggerKind::Admin),
                        AcmeCommand::StartupCheck { key } => (key, TriggerKind::Startup),
                    };
                    self.enqueue_check(key, trigger);
                }
                Some(result) = self.jobs.next(), if !self.jobs.is_empty() => {
                    self.handle_job_result(result);
                }
                _ = tokio::time::sleep_until(deadline) => {
                    self.fire_due_deadlines();
                }
            }
        }

        tracing::info!(component = "acme_service", "Worker stopped");
    }

    fn enqueue_check(&mut self, key: String, trigger: TriggerKind) {
        let state = self.schedules.entry(key.clone()).or_default();
        let cancel_active = trigger == TriggerKind::Resource;
        let trigger = claim_trigger(state, trigger);
        if state.busy {
            if cancel_active
                && !state
                    .certificate_issued
                    .as_ref()
                    .is_some_and(|issued| issued.load(Ordering::Acquire))
            {
                if let Some(cancel) = &state.active_cancel {
                    cancel.cancel();
                }
            }
            state.merge_pending(trigger);
            return;
        }
        state.busy = true;
        let client = self.client.clone();
        let runtime = self.runtime.clone();
        let leader = self.leader.clone();
        let generation = self.generation;
        let cancel = self.cancel.child_token();
        state.active_cancel = Some(cancel.clone());
        self.jobs.push(Box::pin(async move {
            let outcome = check_resource(client, runtime, leader, generation, cancel, &key, trigger).await;
            WorkerJobResult::Checked { key, outcome }
        }));
    }

    fn handle_job_result(&mut self, result: WorkerJobResult) {
        match result {
            WorkerJobResult::Checked { key, outcome } => match outcome {
                CheckOutcome::Start(plan) => {
                    let attempt_id = plan.attempt.attempt_id.clone();
                    let state = self.schedules.entry(key.clone()).or_default();
                    state.active_cancel = Some(plan.attempt.cancel.clone());
                    state.active_attempt_id = Some(attempt_id.clone());
                    state.certificate_issued = Some(plan.attempt.certificate_issued.clone());
                    self.jobs.push(Box::pin(async move {
                        let outcome = run_attempt(&plan).await;
                        WorkerJobResult::AttemptFinished {
                            key,
                            attempt_id,
                            outcome,
                        }
                    }));
                }
                outcome => self.finish_key(key, outcome),
            },
            WorkerJobResult::AttemptFinished {
                key,
                attempt_id,
                outcome,
            } => {
                let state = self.schedules.entry(key.clone()).or_default();
                if state.active_attempt_id.as_deref() != Some(attempt_id.as_str()) {
                    tracing::warn!(component = "acme_service", key = %key, attempt_id = %attempt_id,
                        "Ignoring stale ACME attempt completion");
                    return;
                }
                state.busy = false;
                state.active_cancel = None;
                state.active_attempt_id = None;
                state.certificate_issued = None;
                match outcome {
                    AttemptOutcome::Success(delay) => state.set_deadline(delay, TriggerKind::Renewal),
                    AttemptOutcome::Failed(Some(delay)) => state.set_deadline(delay, TriggerKind::Retry),
                    AttemptOutcome::Failed(None) => {}
                    AttemptOutcome::Retired => state.set_deadline(ELIGIBILITY_RECHECK_DELAY, TriggerKind::Eligibility),
                }
                tracing::debug!(component = "acme_service", key = %key, attempt_id = %attempt_id, "ACME attempt settled");
                self.start_pending(key);
            }
        }
    }

    fn finish_key(&mut self, key: String, outcome: CheckOutcome) {
        if matches!(outcome, CheckOutcome::Deleted) {
            let pending = take_deleted_pending(&mut self.schedules, &key);
            if let Some(trigger) = pending {
                self.enqueue_check(key, trigger);
            }
            return;
        }
        let state = self.schedules.entry(key.clone()).or_default();
        state.busy = false;
        state.active_cancel = None;
        state.active_attempt_id = None;
        state.certificate_issued = None;
        match outcome {
            CheckOutcome::Recheck(delay, trigger) => state.set_deadline(delay, trigger),
            CheckOutcome::Wait(delay, trigger) => state.set_deadline(delay, trigger),
            CheckOutcome::Deleted | CheckOutcome::Idle | CheckOutcome::Start(_) => {}
        }
        self.start_pending(key);
    }

    fn start_pending(&mut self, key: String) {
        let pending = self.schedules.get_mut(&key).and_then(|state| state.pending.take());
        if let Some(trigger) = pending {
            self.enqueue_check(key, trigger);
        }
    }

    fn fire_due_deadlines(&mut self) {
        let now = Instant::now();
        let due: Vec<_> = self
            .schedules
            .iter_mut()
            .filter_map(|(key, state)| {
                state
                    .deadline
                    .filter(|(deadline, _)| *deadline <= now)
                    .map(|(_, trigger)| {
                        state.deadline = None;
                        (key.clone(), trigger)
                    })
            })
            .collect();
        for (key, trigger) in due {
            self.enqueue_check(key, trigger);
        }
    }
}

fn claim_trigger(state: &mut ResourceSchedule, incoming: TriggerKind) -> TriggerKind {
    let scheduled = state.deadline.take().map(|(_, trigger)| trigger);
    if incoming == TriggerKind::Admin || scheduled == Some(TriggerKind::Admin) {
        TriggerKind::Admin
    } else {
        incoming
    }
}

fn take_deleted_pending(schedules: &mut HashMap<String, ResourceSchedule>, key: &str) -> Option<TriggerKind> {
    schedules.remove(key).and_then(|state| state.pending)
}

async fn check_resource(
    client: Client,
    runtime: Arc<ControllerRuntime>,
    leader: Option<LeaderHandle>,
    generation: u64,
    cancel: CancellationToken,
    key: &str,
    trigger: TriggerKind,
) -> CheckOutcome {
    let (namespace, name) = parse_resource_key(key);
    let api: Api<EdgionAcme> = Api::namespaced(client.clone(), &namespace);
    let acme = match api.get(&name).await {
        Ok(acme) => acme,
        Err(kube::Error::Api(response)) if response.code == 404 => return CheckOutcome::Deleted,
        Err(error) => {
            tracing::warn!(
                component = "acme_service",
                key = %key,
                error = %error,
                "Cannot read EdgionAcme; retaining scheduler state for retry"
            );
            return CheckOutcome::Recheck(CONTROL_PLANE_RECHECK_DELAY, trigger);
        }
    };
    if acme.metadata.deletion_timestamp.is_some() {
        return CheckOutcome::Deleted;
    }
    match evaluate_issuance_eligibility(&acme, key, &runtime) {
        EligibilityDecision::Eligible => {}
        EligibilityDecision::AuthoritativelyIneligible => return CheckOutcome::Idle,
        EligibilityDecision::Transient => return CheckOutcome::Recheck(ELIGIBILITY_RECHECK_DELAY, trigger),
    }

    let Some(resource_generation) = acme.metadata.generation else {
        return CheckOutcome::Recheck(ELIGIBILITY_RECHECK_DELAY, trigger);
    };
    let persisted = acme.status.as_ref().and_then(|status| status.scheduler.clone());
    let same_generation = persisted
        .as_ref()
        .is_some_and(|status| status.observed_generation == resource_generation);
    let attempts_used = persisted_attempts(trigger, resource_generation, persisted.as_ref());
    let phase = acme
        .status
        .as_ref()
        .map(|status| status.phase.clone())
        .unwrap_or(AcmeCertPhase::Pending);
    let leader_fence = match leader.as_ref() {
        Some(leader) => match leader.current_fence() {
            Some(fence) => Some(fence),
            None => return CheckOutcome::Recheck(CONTROL_PLANE_RECHECK_DELAY, trigger),
        },
        None => None,
    };
    let Some(serving_attachments) = admitted_serving_attachments(key, &runtime) else {
        return CheckOutcome::Recheck(ELIGIBILITY_RECHECK_DELAY, trigger);
    };
    let attempt = AcmeAttempt {
        attempt_id: Uuid::new_v4().to_string(),
        client,
        acme,
        runtime,
        leader,
        leader_fence,
        serving_attachments,
        certificate_issued: Arc::new(AtomicBool::new(false)),
        generation,
        cancel,
    };
    let mut attempts_used = attempts_used;
    if matches!(phase, AcmeCertPhase::Ready) && persisted.is_some() {
        if let Err(error) = patch_scheduler_status(&api, &attempt, None).await {
            tracing::warn!(component = "acme_service", key = %key, error = %error,
                "Failed to clear a completed ACME scheduler checkpoint");
            return CheckOutcome::Recheck(CONTROL_PLANE_RECHECK_DELAY, trigger);
        }
        attempts_used = 0;
    }
    if matches!(phase, AcmeCertPhase::Ready) && !needs_renewal(&attempt.acme) {
        let delay = spec_duration_or(
            &attempt.acme.spec.renewal.check_interval,
            "checkInterval",
            Duration::from_secs(86400),
        );
        return CheckOutcome::Wait(delay, TriggerKind::Renewal);
    }
    if !trigger.resets_retry_budget() && same_generation && !matches!(phase, AcmeCertPhase::Ready) {
        if let Some(next_retry_at) = persisted.as_ref().and_then(|status| status.next_retry_at.as_deref()) {
            if let Ok(next) = chrono::DateTime::parse_from_rfc3339(next_retry_at) {
                let now = chrono::Utc::now();
                if next > now {
                    let delay = next
                        .signed_duration_since(now.fixed_offset())
                        .to_std()
                        .unwrap_or(CONTROL_PLANE_RECHECK_DELAY);
                    return CheckOutcome::Wait(delay, TriggerKind::Retry);
                }
            }
        }
    }

    if attempts_used >= MAX_RETRY_ATTEMPTS {
        if !matches!(phase, AcmeCertPhase::Failed) {
            if let Err(error) = update_acme_status(&api, &attempt, &name, AcmeCertPhase::Failed, |status| {
                status.last_failure_reason = Some("ACME attempt budget exhausted after an interrupted attempt; edit the resource or use the Admin trigger to retry".to_string());
                status.last_failure_time = Some(now_rfc3339());
            }).await {
                tracing::warn!(component = "acme_service", key = %key, error = %error,
                    "Failed to persist exhausted ACME attempt state; retrying without contacting the CA");
                return CheckOutcome::Recheck(CONTROL_PLANE_RECHECK_DELAY, trigger);
            }
        }
        tracing::warn!(
            component = "acme_service",
            key = %key,
            attempts = attempts_used,
            "ACME retry limit reached; edit the resource or use the Admin trigger to reset"
        );
        return CheckOutcome::Idle;
    }
    if attempt
        .acme
        .status
        .as_ref()
        .and_then(|status| status.last_failure_reason.as_deref())
        .is_some_and(|reason| reason.starts_with("EdgionAcme:"))
    {
        return CheckOutcome::Idle;
    }

    let attempts_used = attempts_used.saturating_add(1);
    let checkpoint = AcmeSchedulerStatus {
        observed_generation: resource_generation,
        attempt_id: Some(attempt.attempt_id.clone()),
        attempts_used,
        attempt_started_at: Some(now_rfc3339()),
        next_retry_at: None,
    };
    if let Err(error) = patch_scheduler_status(&api, &attempt, Some(&checkpoint)).await {
        tracing::warn!(component = "acme_service", key = %key, error = %error,
            "Failed to reserve ACME attempt; no CA request was started");
        return CheckOutcome::Recheck(CONTROL_PLANE_RECHECK_DELAY, trigger);
    }
    let is_renewal = matches!(phase, AcmeCertPhase::Ready | AcmeCertPhase::Renewing);
    let fail_backoff_base = spec_duration_or(
        &attempt.acme.spec.renewal.fail_backoff,
        "failBackoff",
        Duration::from_secs(300),
    )
    .as_secs();
    CheckOutcome::Start(Box::new(AttemptPlan {
        attempt,
        is_renewal,
        fail_backoff_base,
        attempts_used,
    }))
}

fn persisted_attempts(trigger: TriggerKind, resource_generation: i64, persisted: Option<&AcmeSchedulerStatus>) -> u32 {
    if trigger.resets_retry_budget() || persisted.is_none_or(|status| status.observed_generation != resource_generation)
    {
        0
    } else {
        persisted.map_or(0, |status| status.attempts_used)
    }
}

fn needs_renewal(acme: &EdgionAcme) -> bool {
    let Some(status) = &acme.status else {
        return true;
    };
    let Some(not_after) = &status.certificate_not_after else {
        return true;
    };
    let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(not_after) else {
        return true;
    };
    let renew_before = chrono::Duration::from_std(spec_duration_or(
        &acme.spec.renewal.renew_before,
        "renewBefore",
        Duration::from_secs(720 * 3600),
    ))
    .unwrap_or_else(|_| chrono::Duration::days(30));
    chrono::Utc::now() >= expiry - renew_before
}

async fn run_attempt(plan: &AttemptPlan) -> AttemptOutcome {
    let attempt = &plan.attempt;
    let (namespace, name) = attempt.resource_names();
    let api: Api<EdgionAcme> = Api::namespaced(attempt.client(), &namespace);
    let issued = issue_acme_resource(&api, attempt, plan.is_renewal).await;
    let (certificate, account_uri) = match issued {
        Ok(result) => result,
        Err(error) => {
            if attempt.await_current_resource(&api).await.is_err() {
                return AttemptOutcome::Retired;
            }
            let next_delay = (plan.attempts_used < MAX_RETRY_ATTEMPTS)
                .then(|| exponential_backoff(plan.fail_backoff_base, plan.attempts_used.saturating_sub(1)));
            let next_retry_at = next_delay.and_then(|delay| {
                chrono::Duration::from_std(delay)
                    .ok()
                    .map(|delay| (chrono::Utc::now() + delay).to_rfc3339())
            });
            let checkpoint = AcmeSchedulerStatus {
                observed_generation: attempt.acme.metadata.generation.unwrap_or_default(),
                attempt_id: Some(attempt.attempt_id.clone()),
                attempts_used: plan.attempts_used,
                attempt_started_at: Some(now_rfc3339()),
                next_retry_at,
            };
            loop {
                if attempt.await_current_resource(&api).await.is_err() {
                    return AttemptOutcome::Retired;
                }
                let status_result = update_acme_status(&api, attempt, &name, AcmeCertPhase::Failed, |status| {
                    status.last_failure_reason = Some(format!("{error:#}"));
                    status.last_failure_time = Some(now_rfc3339());
                })
                .await;
                let checkpoint_result = patch_scheduler_status(&api, attempt, Some(&checkpoint)).await;
                if status_result.is_ok() && checkpoint_result.is_ok() {
                    break;
                }
                tracing::warn!(component = "acme_service", key = %attempt.key(),
                    "Failed to persist ACME failure checkpoint; retrying without another CA attempt");
                if attempt.wait_or_cancel(CONTROL_PLANE_RECHECK_DELAY).await.is_err() {
                    return AttemptOutcome::Retired;
                }
            }
            return AttemptOutcome::Failed(next_delay);
        }
    };

    loop {
        match publish_certificate(attempt.client_ref(), &api, attempt, &certificate, &account_uri).await {
            Ok(()) => break,
            Err(error) => {
                if attempt.await_current_resource(&api).await.is_err() {
                    return AttemptOutcome::Retired;
                }
                tracing::warn!(component = "acme_service", key = %attempt.key(), error = %error,
                    "Certificate was issued but publication failed; retrying publication without contacting the CA");
                if attempt.wait_or_cancel(CONTROL_PLANE_RECHECK_DELAY).await.is_err() {
                    return AttemptOutcome::Retired;
                }
            }
        }
    }
    loop {
        if attempt.await_current_resource(&api).await.is_err() {
            return AttemptOutcome::Retired;
        }
        match patch_scheduler_status(&api, attempt, None).await {
            Ok(()) => break,
            Err(error) => {
                tracing::warn!(component = "acme_service", key = %attempt.key(), error = %error,
                    "Certificate is published but the scheduler checkpoint could not be cleared; retrying");
                if attempt.wait_or_cancel(CONTROL_PLANE_RECHECK_DELAY).await.is_err() {
                    return AttemptOutcome::Retired;
                }
            }
        }
    }
    let renewal_delay = spec_duration_or(
        &attempt.acme.spec.renewal.check_interval,
        "checkInterval",
        Duration::from_secs(86400),
    );
    tracing::info!(component = "acme_service", key = %attempt.key(), "ACME flow completed successfully");
    AttemptOutcome::Success(renewal_delay)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EligibilityDecision {
    Eligible,
    AuthoritativelyIneligible,
    Transient,
}

fn evaluate_issuance_eligibility(raw: &EdgionAcme, key: &str, runtime: &ControllerRuntime) -> EligibilityDecision {
    // Source ownership changes requeue raw ACME resources, including ones never
    // published here. A non-owner must not spin an eligibility retry timer.
    if !acme_is_owned(raw, runtime) {
        return EligibilityDecision::AuthoritativelyIneligible;
    }
    let Some(uid) = raw.metadata.uid.as_deref() else {
        return EligibilityDecision::Transient;
    };
    let Some(generation) = raw.metadata.generation else {
        return EligibilityDecision::Transient;
    };
    let Some(processor) = runtime.processor_registry.get("EdgionAcme") else {
        return EligibilityDecision::Transient;
    };
    let Some(processed) = processor
        .get_cached_any(key)
        .and_then(|value| value.downcast::<EdgionAcme>().ok())
    else {
        return EligibilityDecision::Transient;
    };
    if processed.metadata.uid.as_deref() != Some(uid) || processed.metadata.generation != Some(generation) {
        return EligibilityDecision::Transient;
    }

    let Some(status) = processed.spec.current_status.as_ref() else {
        return EligibilityDecision::Transient;
    };
    let accepted = status.conditions.iter().find(|condition| condition.type_ == "Accepted");
    let Some(accepted) = accepted else {
        return EligibilityDecision::Transient;
    };
    if accepted.observed_generation != Some(generation) {
        return EligibilityDecision::Transient;
    }
    if accepted.status != "True" {
        return EligibilityDecision::AuthoritativelyIneligible;
    }

    let resolved_refs = status
        .conditions
        .iter()
        .find(|condition| condition.type_ == "ResolvedRefs");
    let Some(resolved_refs) = resolved_refs else {
        return EligibilityDecision::Transient;
    };
    if resolved_refs.observed_generation != Some(generation) {
        return EligibilityDecision::Transient;
    }
    let refs_eligible = resolved_refs.status == "True"
        || (resolved_refs.status == "Unknown" && resolved_refs.reason == "NeedsRegistration");
    if !refs_eligible {
        return EligibilityDecision::AuthoritativelyIneligible;
    }

    let Some(processed_attachments) = processed.spec.resolved_listener_attachments.as_deref() else {
        return EligibilityDecision::AuthoritativelyIneligible;
    };
    if processed_attachments.is_empty()
        || processed_attachments.len() > MAX_ACME_RESOLVED_ATTACHMENTS
        || processed_attachments
            .iter()
            .any(|attachment| !attachment.gateway_proof.is_resolved())
    {
        return EligibilityDecision::AuthoritativelyIneligible;
    }

    let gateway_ref = match &raw.spec.challenge {
        AcmeChallenge::Http01 { gateway_ref } => gateway_ref,
    };
    let live_attachments = match resolve_acme_listener_attachments(
        gateway_ref,
        raw.metadata.namespace.as_deref().unwrap_or("default"),
        runtime,
    ) {
        Ok(attachments) => attachments,
        // A projected Gateway can disappear briefly while a publication epoch
        // changes. Resource notifications and all other trigger sources retain
        // one convergence check until the processed proof catches up.
        Err(AcmeGatewayRefError::ParentNotFound { .. } | AcmeGatewayRefError::UnresolvedGateway) => {
            return EligibilityDecision::Transient;
        }
        // Invalid operator input or a currently denied/unsupported live target
        // is authoritative. Its dependency requeue will notify again if the
        // condition later becomes eligible.
        Err(
            AcmeGatewayRefError::InvalidKind { .. }
            | AcmeGatewayRefError::RefNotPermitted { .. }
            | AcmeGatewayRefError::NoMatchingListener
            | AcmeGatewayRefError::NoHttpListener
            | AcmeGatewayRefError::TooManyListeners,
        ) => return EligibilityDecision::AuthoritativelyIneligible,
    };
    if live_attachments.as_slice() != processed_attachments {
        return EligibilityDecision::Transient;
    }

    EligibilityDecision::Eligible
}

fn admitted_serving_attachments(key: &str, runtime: &ControllerRuntime) -> Option<Vec<ResolvedListenerAttachment>> {
    runtime
        .processor_registry
        .get("EdgionAcme")?
        .get_cached_any(key)?
        .downcast::<EdgionAcme>()
        .ok()?
        .spec
        .resolved_listener_attachments
        .clone()
}

/// After issuance, a Gateway certificate reload may change its digest and epoch.
/// Keep the selected listener set and Gateway UID/spec generation fixed, while
/// eligibility independently requires the new proof to be fully published/live.
fn same_serving_selection(previous: &[ResolvedListenerAttachment], current: &[ResolvedListenerAttachment]) -> bool {
    previous.len() == current.len()
        && previous.iter().all(|old| {
            current.iter().any(|new| {
                old.gateway_namespace == new.gateway_namespace
                    && old.gateway_name == new.gateway_name
                    && old.listener_name == new.listener_name
                    && old.listener_key == new.listener_key
                    && old.gateway_proof.instance == new.gateway_proof.instance
                    && old.gateway_proof.generation == new.gateway_proof.generation
            })
        })
}

// ============================================================================
// ACME Flow - The core certificate issuance/renewal logic
// ============================================================================

/// Authority captured for one admitted issuance attempt.
struct AcmeAttempt {
    attempt_id: String,
    client: Client,
    acme: EdgionAcme,
    runtime: Arc<ControllerRuntime>,
    leader: Option<LeaderHandle>,
    leader_fence: Option<LeaderFence>,
    serving_attachments: Vec<ResolvedListenerAttachment>,
    certificate_issued: Arc<AtomicBool>,
    generation: u64,
    cancel: CancellationToken,
}

#[derive(Debug, thiserror::Error)]
enum AttemptAuthorityError {
    #[error("ACME attempt authority was lost: {0:#}")]
    Lost(anyhow::Error),
    #[error("ACME attempt authority is temporarily uncertain: {0:#}")]
    Uncertain(anyhow::Error),
}

impl AcmeAttempt {
    fn key(&self) -> String {
        let (namespace, name) = self.resource_names();
        format!("{namespace}/{name}")
    }

    fn resource_names(&self) -> (String, String) {
        (
            self.acme.metadata.namespace.as_deref().unwrap_or("default").to_string(),
            self.acme.metadata.name.as_deref().unwrap_or_default().to_string(),
        )
    }

    fn client(&self) -> Client {
        self.client.clone()
    }

    fn client_ref(&self) -> &Client {
        &self.client
    }

    async fn validate_leader_typed(&self) -> std::result::Result<(), AttemptAuthorityError> {
        if self.cancel.is_cancelled() {
            return Err(AttemptAuthorityError::Lost(anyhow::anyhow!(
                "ACME attempt was cancelled"
            )));
        }
        if self.generation == 0 || current_generation() != self.generation {
            return Err(AttemptAuthorityError::Lost(anyhow::anyhow!(
                "ACME service generation is no longer current"
            )));
        }
        match (&self.leader, &self.leader_fence) {
            (Some(leader), Some(fence)) => match leader.validate_fence_typed(fence).await {
                LeaderFenceValidation::Current => {}
                LeaderFenceValidation::Lost => {
                    return Err(AttemptAuthorityError::Lost(anyhow::anyhow!(
                        "ACME leadership fence is no longer current"
                    )))
                }
                LeaderFenceValidation::Uncertain(error) => {
                    return Err(AttemptAuthorityError::Uncertain(
                        anyhow::Error::new(error).context("Cannot confirm ACME leadership fence"),
                    ))
                }
            },
            (None, None) if cfg!(test) => {}
            _ => {
                return Err(AttemptAuthorityError::Lost(anyhow::anyhow!(
                    "ACME attempt has no leadership fence"
                )))
            }
        }
        if self.cancel.is_cancelled() || current_generation() != self.generation {
            return Err(AttemptAuthorityError::Lost(anyhow::anyhow!(
                "ACME service generation changed during authority validation"
            )));
        }
        Ok(())
    }

    async fn current_resource_typed(
        &self,
        api: &Api<EdgionAcme>,
    ) -> std::result::Result<EdgionAcme, AttemptAuthorityError> {
        self.validate_leader_typed().await?;
        let name = self
            .acme
            .metadata
            .name
            .as_deref()
            .ok_or_else(|| AttemptAuthorityError::Lost(anyhow::anyhow!("EdgionAcme has no name")))?;
        let current = match api.get(name).await {
            Ok(current) => current,
            Err(kube::Error::Api(response)) if response.code == 404 => {
                return Err(AttemptAuthorityError::Lost(anyhow::anyhow!(
                    "EdgionAcme no longer exists"
                )))
            }
            Err(error) => {
                return Err(AttemptAuthorityError::Uncertain(
                    anyhow::Error::new(error).context("Cannot confirm ACME publication authority"),
                ))
            }
        };
        let key = format!(
            "{}/{}",
            self.acme.metadata.namespace.as_deref().unwrap_or("default"),
            name
        );
        if self.cancel.is_cancelled()
            || current_generation() != self.generation
            || current.metadata.deletion_timestamp.is_some()
            || current.metadata.uid != self.acme.metadata.uid
            || current.metadata.generation != self.acme.metadata.generation
        {
            return Err(AttemptAuthorityError::Lost(anyhow::anyhow!(
                "ACME resource identity or generation changed"
            )));
        }
        match evaluate_issuance_eligibility(&current, &key, &self.runtime) {
            EligibilityDecision::Eligible => {}
            EligibilityDecision::AuthoritativelyIneligible => {
                return Err(AttemptAuthorityError::Lost(anyhow::anyhow!(
                    "ACME serving authority was revoked"
                )))
            }
            EligibilityDecision::Transient => {
                return Err(AttemptAuthorityError::Uncertain(anyhow::anyhow!(
                    "ACME serving authority is not yet converged"
                )))
            }
        }
        let current_attachments = admitted_serving_attachments(&key, &self.runtime);
        let attachments_match = current_attachments.as_deref().is_some_and(|current| {
            if self.certificate_issued.load(Ordering::Acquire) {
                same_serving_selection(&self.serving_attachments, current)
            } else {
                current == self.serving_attachments
            }
        });
        if !attachments_match {
            return Err(AttemptAuthorityError::Lost(anyhow::anyhow!(
                "ACME serving proof changed"
            )));
        }
        self.validate_leader_typed().await?;
        Ok(current)
    }

    async fn current_resource(&self, api: &Api<EdgionAcme>) -> Result<EdgionAcme> {
        self.current_resource_typed(api).await.map_err(anyhow::Error::new)
    }

    async fn await_current_resource(&self, api: &Api<EdgionAcme>) -> Result<EdgionAcme> {
        loop {
            match self.current_resource_typed(api).await {
                Ok(current) => return Ok(current),
                Err(AttemptAuthorityError::Lost(error)) => return Err(error),
                Err(AttemptAuthorityError::Uncertain(error)) => {
                    tracing::warn!(component = "acme_service", key = %self.key(), error = %error,
                        "ACME attempt authority is temporarily uncertain; retrying in place");
                    self.wait_or_cancel(CONTROL_PLANE_RECHECK_DELAY).await?;
                }
            }
        }
    }

    async fn wait_or_cancel(&self, delay: Duration) -> Result<()> {
        tokio::select! {
            _ = tokio::time::sleep(delay) => Ok(()),
            _ = self.cancel.cancelled() => anyhow::bail!("ACME attempt was cancelled"),
        }
    }
}

#[async_trait::async_trait]
impl AcmeOperationGuard for AcmeAttempt {
    async fn check(&self, _operation: &'static str) -> Result<()> {
        let (namespace, _) = self.resource_names();
        let api: Api<EdgionAcme> = Api::namespaced(self.client.clone(), &namespace);
        self.await_current_resource(&api).await.map(|_| ())
    }

    async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }
}

/// Run the CA-facing part of an attempt and retain the certificate in memory so
/// publication retries never repeat issuance within this service generation.
async fn issue_acme_resource(
    api: &Api<EdgionAcme>,
    attempt: &AcmeAttempt,
    is_renewal: bool,
) -> Result<(AcmeCertificateResult, String)> {
    let acme = &attempt.acme;
    let name = acme.metadata.name.as_deref().context("EdgionAcme has no name")?;

    // 1. Update status to Issuing/Renewing
    let phase = if is_renewal {
        AcmeCertPhase::Renewing
    } else {
        AcmeCertPhase::Issuing
    };
    update_acme_status(api, attempt, name, phase, |_| {}).await?;

    // 2. Create or restore ACME account
    let acme_client = get_or_create_acme_account(attempt, acme).await?;

    // 3. Execute HTTP-01 and obtain the certificate
    let cert_result = execute_http01_flow(attempt, api, name, &acme_client, acme).await?;

    let account_uri = acme_client.account_id().to_string();
    Ok((cert_result, account_uri))
}

/// Kept separate from CA IO so publication after a resource change is testable.
async fn publish_certificate(
    client: &Client,
    api: &Api<EdgionAcme>,
    attempt: &AcmeAttempt,
    cert_result: &AcmeCertificateResult,
    account_uri: &str,
) -> Result<()> {
    let acme = &attempt.acme;
    let namespace = acme.metadata.namespace.as_deref().unwrap_or("default");
    let name = acme.metadata.name.as_deref().context("EdgionAcme has no name")?;
    let key = format!("{}/{}", namespace, name);

    // 4. Store certificate in K8s Secret
    let secret_ns = acme.get_secret_namespace();
    attempt.current_resource(api).await?;
    create_or_update_cert_secret(
        client,
        attempt,
        &secret_ns,
        &acme.spec.storage.secret_name,
        cert_result,
        name,
    )
    .await?;

    // 5. Create/update EdgionTls if enabled
    if acme.spec.auto_edgion_tls.enabled {
        attempt.current_resource(api).await?;
        create_or_update_edgion_tls(client, attempt, acme).await?;
    }

    // 6. Parse certificate info for status
    let cert_info = parse_cert_info(&cert_result.certificate_pem);

    // 7. Update status to Ready
    update_acme_status(api, attempt, name, AcmeCertPhase::Ready, |status| {
        status.certificate_serial = cert_info.serial.clone();
        status.certificate_not_after = cert_info.not_after.clone();
        status.last_renewal_time = Some(now_rfc3339());
        status.last_failure_reason = None;
        status.last_failure_time = None;
        status.secret_name = Some(acme.spec.storage.secret_name.clone());
        if acme.spec.auto_edgion_tls.enabled {
            status.edgion_tls_name = Some(acme.get_edgion_tls_name());
        }
        status.account_uri = Some(account_uri.to_string());
    })
    .await?;

    tracing::info!(
        component = "acme_service",
        key = %key,
        secret = %acme.spec.storage.secret_name,
        not_after = ?cert_info.not_after,
        "Certificate issued and stored successfully"
    );

    Ok(())
}

// ============================================================================
// HTTP-01 Challenge Flow
// ============================================================================

/// Propagation wait time for Gateway to load challenge tokens via gRPC (seconds).
///
/// Timeline: Controller patches CRD → K8s watcher fires (~100ms) →
/// ResourceProcessor → ServerCache → gRPC watch push (~100ms) →
/// Gateway AcmeConfHandler → ChallengeStore.
/// Total: typically < 2s, using 5s for safety margin.
const CHALLENGE_PROPAGATION_WAIT_SECS: u64 = 5;

async fn execute_http01_flow(
    attempt: &AcmeAttempt,
    api: &Api<EdgionAcme>,
    name: &str,
    acme_client: &AcmeClient,
    acme: &EdgionAcme,
) -> Result<AcmeCertificateResult> {
    tracing::info!(
        component = "acme_service",
        name = %name,
        domains = ?acme.spec.domains,
        "Starting HTTP-01 challenge flow"
    );

    // Phase 1: Create order and extract challenge tokens (does NOT notify ACME server yet)
    let (pending, mut order_ctx) = acme_client
        .prepare_http01_order(&acme.spec.domains, attempt)
        .await
        .context("Failed to prepare HTTP-01 ACME order")?;

    if pending.is_empty() {
        tracing::info!(
            component = "acme_service",
            name = %name,
            "All authorizations already valid, skipping challenge"
        );
    } else {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Phase 2: Push challenge tokens to CRD → K8s watcher → ServerCache → gRPC → Gateway
        let challenges: Vec<ActiveHttpChallenge> = pending
            .iter()
            .map(|p| ActiveHttpChallenge {
                domain: p.domain.clone(),
                token: p.token.clone(),
                key_authorization: p.key_authorization.clone(),
                expire_at: now_secs + 600, // 10 minutes
            })
            .collect();

        tracing::info!(
            component = "acme_service",
            name = %name,
            challenge_count = challenges.len(),
            "Pushing HTTP-01 challenge tokens to CRD"
        );

        patch_active_challenges(api, attempt, name, Some(&challenges)).await?;

        // Phase 3: Wait for Gateway to receive tokens via gRPC watch stream
        tracing::debug!(
            component = "acme_service",
            name = %name,
            wait_secs = CHALLENGE_PROPAGATION_WAIT_SECS,
            "Waiting for challenge tokens to propagate to Gateway"
        );
        attempt
            .wait_or_cancel(Duration::from_secs(CHALLENGE_PROPAGATION_WAIT_SECS))
            .await?;

        // Phase 4: NOW tell the ACME server to start validation
        // (Gateway is ready to serve challenge responses at this point)
        acme_client
            .activate_http01_challenges(&mut order_ctx, attempt)
            .await
            .context("Failed to activate HTTP-01 challenges")?;

        tracing::info!(
            component = "acme_service",
            name = %name,
            "ACME server notified, waiting for challenge validation"
        );
    }

    // Phase 5: Poll for validation, finalize order, retrieve certificate
    let cert_result = acme_client
        .complete_http01_order(order_ctx, &acme.spec.domains, &acme.spec.key_type, attempt)
        .await;

    // Secret publication can requeue this Gateway and rotate its proof epoch.
    // Preserve the issued certificate through that feedback while every write
    // still validates the source and the semantic serving selection.
    if cert_result.is_ok() {
        attempt.certificate_issued.store(true, Ordering::Release);
    }

    // Phase 6: Attempt cleanup on success or failure, but never clear a successor's tokens.
    // Rejected cleanup leaves old tokens subject to their expiry (up to 600s).
    if let Err(e) = patch_active_challenges(api, attempt, name, None).await {
        tracing::warn!(
            component = "acme_service",
            name = %name,
            error = %e,
            "Failed to clear challenge tokens (non-fatal)"
        );
    }

    cert_result.context("Failed to complete HTTP-01 ACME order")
}

/// Fetch the EAB HMAC key from `external_account_binding.key_secret_ref`.
/// Returns `Ok(None)` when no EAB is configured, `Ok(Some(key))` when present,
/// and `Err` when configured but the Secret / `hmacKey` entry is missing or
/// malformed (which blocks registration — EAB is a CA-side precondition).
async fn fetch_eab_hmac(client: &Client, acme: &EdgionAcme, owner_ns: &str) -> Result<Option<String>> {
    let Some(eab) = acme.spec.external_account_binding.as_ref() else {
        return Ok(None);
    };
    let sref = &eab.key_secret_ref;
    let ns = sref.effective_namespace(owner_ns);
    let api: Api<Secret> = Api::namespaced(client.clone(), ns);
    let secret = api
        .get(&sref.name)
        .await
        .with_context(|| format!("Failed to fetch EAB key Secret {}/{}", ns, sref.name))?;
    let data = secret.data.as_ref().context("EAB key Secret has no data")?;
    let bytes = data.get("hmacKey").context("EAB key Secret missing 'hmacKey' entry")?;
    let hmac = String::from_utf8(bytes.0.clone()).context("EAB hmacKey is not valid UTF-8")?;
    Ok(Some(hmac))
}

// ============================================================================
// ACME Account Management
// ============================================================================

/// Get or create an ACME account for the given resource.
///
/// Uses `spec.privateKeySecretRef` and the `accountCredentials` data key (see `ACME_ACCOUNT_KEY`).
/// - Secret not found: register, then create Secret (with ownerReferences when uid is set).
/// - Secret exists with valid `accountCredentials` (or legacy `credentials`):
///   restore — **no overwrite** (backup / operator restore).
/// - Secret exists but missing those keys: register, then **merge-patch** `accountCredentials` only
///   (self-managed empty Secret).
/// - Key present but JSON invalid: **error** (operator must fix; we do not re-register over it).
async fn get_or_create_acme_account(attempt: &AcmeAttempt, acme: &EdgionAcme) -> Result<AcmeClient> {
    let client = attempt.client_ref();
    let owner_ns = acme.metadata.namespace.as_deref().unwrap_or("default");
    let acme_name = acme.metadata.name.as_deref().context("EdgionAcme has no name")?;
    let acme_uid = acme.metadata.uid.as_deref();

    let secret_name = &acme.spec.private_key_secret_ref.name;
    let secret_ns = acme.spec.private_key_secret_ref.effective_namespace(owner_ns);
    let secret_api: Api<Secret> = Api::namespaced(client.clone(), secret_ns);

    let eab_kid = acme.spec.external_account_binding.as_ref().map(|e| e.key_id.as_str());

    match secret_api.get(secret_name).await {
        Ok(secret) => {
            let data = secret.data.as_ref();
            if let Some(map) = data {
                if let Some(zjson) = extract_account_json_string(map)? {
                    let creds = account_credentials_from_json(&zjson)?;
                    tracing::info!(
                        component = "acme_service",
                        name = %acme_name,
                        "Restored ACME account from Secret"
                    );
                    attempt.check("restore ACME account").await?;
                    return AcmeClient::from_credentials(creds).await;
                }
            }
            // Secret object exists, but no account key yet — pre-created empty (self-managed)
            let eab_hmac = fetch_eab_hmac(client, acme, owner_ns).await?;
            let (acme_client, credentials) = AcmeClient::new(
                &acme.spec.server,
                &acme.spec.email,
                eab_kid,
                eab_hmac.as_deref(),
                attempt,
            )
            .await
            .context("Failed to create ACME account")?;

            let creds_json =
                Zeroizing::new(serde_json::to_string(&credentials).context("Failed to serialize account credentials")?);
            if let Some(existing) =
                persist_registered_account(attempt, &secret_api, secret_name, secret, &creds_json).await?
            {
                attempt.check("restore concurrently persisted ACME account").await?;
                return AcmeClient::from_credentials(existing).await;
            }

            tracing::info!(
                component = "acme_service",
                name = %acme_name,
                account_id = %acme_client.account_id(),
                "New ACME account registered; credentials merge-patched into existing Secret"
            );
            Ok(acme_client)
        }
        Err(kube::Error::Api(resp)) if resp.code == 404 => {
            tracing::info!(
                component = "acme_service",
                name = %acme_name,
                "No account Secret yet; creating ACME account and Secret"
            );
            let eab_hmac = fetch_eab_hmac(client, acme, owner_ns).await?;
            let (acme_client, credentials) = AcmeClient::new(
                &acme.spec.server,
                &acme.spec.email,
                eab_kid,
                eab_hmac.as_deref(),
                attempt,
            )
            .await
            .context("Failed to create ACME account")?;

            let creds_json =
                Zeroizing::new(serde_json::to_string(&credentials).context("Failed to serialize account credentials")?);
            attempt.check("persist ACME account credentials").await?;
            create_account_secret(&secret_api, secret_name, secret_ns, acme_name, acme_uid, &creds_json).await?;

            tracing::info!(
                component = "acme_service",
                name = %acme_name,
                account_id = %acme_client.account_id(),
                "New ACME account created and Secret persisted"
            );
            Ok(acme_client)
        }
        Err(e) => Err(e).context("Failed to get ACME account Secret"),
    }
}

/// Retain newly registered credentials across control-plane uncertainty. An
/// existing valid account always wins a CAS conflict; never overwrite it.
async fn persist_registered_account(
    attempt: &AcmeAttempt,
    api: &Api<Secret>,
    name: &str,
    mut snapshot: Secret,
    credentials: &Zeroizing<String>,
) -> Result<Option<instant_acme::AccountCredentials>> {
    let original_uid = snapshot.metadata.uid.clone();
    loop {
        attempt.check("persist ACME account credentials").await?;
        if snapshot.metadata.uid != original_uid || snapshot.metadata.deletion_timestamp.is_some() {
            anyhow::bail!("ACME account Secret was replaced or is being deleted; retry with its current identity");
        }
        if let Some(json) = snapshot
            .data
            .as_ref()
            .map(extract_account_json_string)
            .transpose()?
            .flatten()
        {
            return account_credentials_from_json(&json).map(Some);
        }
        match patch_account_key_into_existing_secret(api, name, &snapshot, credentials).await {
            Ok(()) => return Ok(None),
            Err(error) => match error.downcast_ref::<kube::Error>() {
                Some(kube::Error::Api(response)) if response.code == 409 || response.code >= 500 => {}
                Some(kube::Error::Api(_)) | None => return Err(error),
                Some(_) => {}
            },
        }
        // A conflict or ambiguous response requires a fresh snapshot before any
        // retry. Delay also bounds contention with a concurrent Secret writer.
        loop {
            attempt.wait_or_cancel(CONTROL_PLANE_RECHECK_DELAY).await?;
            attempt.check("refresh ACME account Secret after conflict").await?;
            match api.get(name).await {
                Ok(current) => {
                    snapshot = current;
                    break;
                }
                Err(kube::Error::Api(response)) if response.code < 500 => return Err(kube::Error::Api(response).into()),
                Err(_) => continue,
            }
        }
    }
}

// ============================================================================
// K8s Resource Operations
// ============================================================================

/// Publish the in-flight HTTP-01 challenge tokens on `status.activeChallenges`
/// via Server-Side Apply against the `/status` subresource.
///
/// Uses a dedicated field manager (`edgion-acme-challenges`) so this writer
/// does not collide with `update_acme_status`, which uses
/// `edgion-acme-controller` and never touches `activeChallenges`.
///
/// Pass `None` (or an empty slice) to clear the field — sending an empty array
/// keeps ownership under our manager and zeroes the observable token list.
async fn patch_active_challenges(
    api: &Api<EdgionAcme>,
    attempt: &AcmeAttempt,
    name: &str,
    challenges: Option<&[ActiveHttpChallenge]>,
) -> Result<()> {
    let current = attempt.await_current_resource(api).await?;
    let resource_version = current
        .metadata
        .resource_version
        .context("EdgionAcme has no resourceVersion")?;
    let active = challenges.unwrap_or(&[]);
    let patch = serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionAcme",
        "metadata": { "name": name, "uid": current.metadata.uid, "resourceVersion": resource_version },
        "status": {
            "activeChallenges": active,
        }
    });

    let pp = PatchParams::apply("edgion-acme-challenges").force();
    api.patch_status(name, &pp, &Patch::Apply(patch))
        .await
        .context("Failed to patch activeChallenges into status")?;

    Ok(())
}

/// Persist or clear the scheduler-owned recovery checkpoint. This dedicated
/// field manager never owns lifecycle, condition, or challenge fields.
async fn patch_scheduler_status(
    api: &Api<EdgionAcme>,
    attempt: &AcmeAttempt,
    scheduler: Option<&AcmeSchedulerStatus>,
) -> Result<()> {
    let current = attempt.current_resource(api).await?;
    let name = current.metadata.name.as_deref().context("EdgionAcme has no name")?;
    let resource_version = current
        .metadata
        .resource_version
        .context("EdgionAcme has no resourceVersion")?;
    let scheduler = scheduler.map(serde_json::to_value).transpose()?;
    let patch = serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionAcme",
        "metadata": {
            "name": name,
            "uid": current.metadata.uid,
            "resourceVersion": resource_version,
        },
        "status": { "scheduler": scheduler },
    });
    let params = PatchParams::apply("edgion-acme-scheduler").force();
    api.patch_status(name, &params, &Patch::Apply(patch))
        .await
        .context("Failed to update ACME scheduler checkpoint")?;
    Ok(())
}

/// Update EdgionAcme status using Server-Side Apply.
///
/// SSA avoids the read-modify-write race condition by letting the K8s API
/// server atomically merge fields. Only the fields we explicitly set will
/// be managed; other controllers' fields are preserved.
async fn update_acme_status<F>(
    api: &Api<EdgionAcme>,
    attempt: &AcmeAttempt,
    name: &str,
    phase: AcmeCertPhase,
    modify: F,
) -> Result<()>
where
    F: FnOnce(&mut EdgionAcmeStatus),
{
    let current = attempt.current_resource(api).await?;
    let resource_version = current
        .metadata
        .resource_version
        .context("EdgionAcme has no resourceVersion")?;
    let mut status = EdgionAcmeStatus {
        phase,
        ..Default::default()
    };
    modify(&mut status);

    let patch = serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionAcme",
        "metadata": { "name": name, "uid": current.metadata.uid, "resourceVersion": resource_version },
        "status": status
    });

    let pp = PatchParams::apply("edgion-acme-controller").force();
    // Try status subresource first; only fall back when the subresource is absent (404/405).
    match api.patch_status(name, &pp, &Patch::Apply(patch.clone())).await {
        Ok(_) => {}
        Err(kube::Error::Api(ref e)) if e.code == 404 => {
            api.patch(name, &pp, &Patch::Apply(patch))
                .await
                .context("Failed to update EdgionAcme status")?;
        }
        Err(e) => return Err(e).context("Failed to patch EdgionAcme status"),
    }

    Ok(())
}

/// Create or update the K8s TLS Secret with the issued certificate
async fn create_or_update_cert_secret(
    client: &Client,
    attempt: &AcmeAttempt,
    namespace: &str,
    name: &str,
    cert: &AcmeCertificateResult,
    acme_resource_name: &str,
) -> Result<()> {
    let api: Api<Secret> = Api::namespaced(client.clone(), namespace);
    let current = api
        .get_opt(name)
        .await
        .context("Failed to read TLS certificate Secret")?;
    attempt.check("publish TLS certificate Secret").await?;

    // Zeroize the intermediate key bytes when this variable goes out of scope.
    // Note: `ByteString` takes ownership of a `Vec<u8>`; after `clone()` below
    // the underlying data in `ByteString` is no longer tracked by Zeroizing,
    // but this ensures the `tls_key_bytes` temp itself is wiped on drop.
    let tls_key_bytes: Zeroizing<Vec<u8>> = Zeroizing::new(cert.private_key_pem.as_bytes().to_vec());
    let secret = Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            uid: current.as_ref().and_then(|secret| secret.metadata.uid.clone()),
            resource_version: current
                .as_ref()
                .and_then(|secret| secret.metadata.resource_version.clone()),
            labels: Some(BTreeMap::from([
                ("edgion.io/managed-by".to_string(), "acme".to_string()),
                ("edgion.io/acme-resource".to_string(), acme_resource_name.to_string()),
            ])),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            (
                "tls.crt".to_string(),
                ByteString(cert.certificate_pem.as_bytes().to_vec()),
            ),
            ("tls.key".to_string(), ByteString(tls_key_bytes.to_vec())),
        ])),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    };

    // `force`: this is the data-plane TLS Secret. Adopting a Secret that already exists
    // (the common `kubectl create secret tls` -> switch-to-ACME migration) leaves its
    // fields owned by another manager; an unforced apply would then 409 on every
    // renewal until the certificate expires and TLS breaks. The controller is the sole
    // authority for the content it writes here. (Guard: cicd/checks/check_ssa_force.py)
    let pp = PatchParams::apply("edgion-acme-controller").force();
    api.patch(name, &pp, &Patch::Apply(secret))
        .await
        .context("Failed to create/update TLS certificate Secret")?;

    tracing::info!(
        component = "acme_service",
        namespace = %namespace,
        secret = %name,
        "Certificate Secret created/updated"
    );

    Ok(())
}

/// Create or update the EdgionTls resource for the issued certificate
async fn create_or_update_edgion_tls(client: &Client, attempt: &AcmeAttempt, acme: &EdgionAcme) -> Result<()> {
    let namespace = acme.metadata.namespace.as_deref().unwrap_or("default");
    let tls_name = acme.get_edgion_tls_name();
    let secret_ns = acme.get_secret_namespace();

    // Build parent_refs
    let parent_refs = acme.spec.auto_edgion_tls.parent_refs.clone();

    // Build EdgionTls JSON for server-side apply
    let mut edgion_tls = serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionTls",
        "metadata": {
            "name": tls_name,
            "namespace": namespace,
            "labels": {
                "edgion.io/managed-by": "acme",
                "edgion.io/acme-resource": acme.metadata.name.as_deref().unwrap_or("unknown"),
            }
        },
        "spec": {
            "hosts": acme.spec.domains,
            "secretRef": {
                "name": acme.spec.storage.secret_name,
                "namespace": secret_ns,
            },
            "parentRefs": parent_refs,
        }
    });

    // Erase the typed resource so the dynamic API retains its declared plural.
    let ar = ApiResource::erase::<EdgionTls>(&());
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);
    let current = api.get_opt(&tls_name).await.context("Failed to read EdgionTls")?;
    attempt.check("publish EdgionTls").await?;

    let metadata = edgion_tls
        .get_mut("metadata")
        .and_then(serde_json::Value::as_object_mut)
        .context("Generated EdgionTls metadata is not an object")?;
    if let Some(current) = current {
        if let Some(uid) = current.metadata.uid {
            metadata.insert("uid".to_string(), serde_json::Value::String(uid));
        }
        if let Some(resource_version) = current.metadata.resource_version {
            metadata.insert(
                "resourceVersion".to_string(),
                serde_json::Value::String(resource_version),
            );
        }
    }

    // `force`: EdgionTls is an operator-facing CRD, so a hand-created or hand-edited
    // instance (owned by kubectl) is expected; without force the apply 409s and ACME
    // can never wire its issued cert up. SSA force only claims the fields this apply
    // writes — operator edits to other fields survive. Mirrors the forced `patch_status`
    // calls above. (Guard: cicd/checks/check_ssa_force.py)
    let pp = PatchParams::apply("edgion-acme-controller").force();
    api.patch(&tls_name, &pp, &Patch::Apply(edgion_tls))
        .await
        .context("Failed to create/update EdgionTls")?;

    tracing::info!(
        component = "acme_service",
        namespace = %namespace,
        edgion_tls = %tls_name,
        "EdgionTls resource created/updated"
    );

    Ok(())
}

// ============================================================================
// Certificate Parsing
// ============================================================================

struct CertInfo {
    serial: Option<String>,
    not_after: Option<String>,
}

/// Parse basic certificate info from PEM
///
/// Note: `not_after` is formatted as RFC 3339 to match `needs_renewal()` which
/// uses `chrono::DateTime::parse_from_rfc3339()`.
fn parse_cert_info(pem: &str) -> CertInfo {
    match x509_parser::pem::parse_x509_pem(pem.as_bytes()) {
        Ok((_, pem_obj)) => match pem_obj.parse_x509() {
            Ok(cert) => {
                let not_after = {
                    let asn1_time = cert.validity().not_after;
                    // Convert ASN1 timestamp to chrono DateTime, then to RFC 3339
                    chrono::DateTime::from_timestamp(asn1_time.timestamp(), 0).map(|dt| dt.to_rfc3339())
                };
                CertInfo {
                    serial: Some(cert.serial.to_str_radix(16)),
                    not_after,
                }
            }
            Err(_) => CertInfo {
                serial: None,
                not_after: None,
            },
        },
        Err(_) => CertInfo {
            serial: None,
            not_after: None,
        },
    }
}

// ============================================================================
// Utility
// ============================================================================

/// Parse a "namespace/name" resource key. Falls back to ("default", key) when '/' is absent.
fn parse_resource_key(key: &str) -> (String, String) {
    let parts: Vec<&str> = key.splitn(2, '/').collect();
    if parts.len() == 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        ("default".to_string(), key.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
        condition_false, condition_reasons, condition_true, condition_types,
    };
    use crate::conf_mgr::sync_runtime::resource_processor::{ProcessorHandler, ResourceProcessor};
    use edgion_resources::prelude_resources::Gateway;
    use edgion_resources::resources::edgion_tls::GatewayAttachmentProof;
    use edgion_resources::resources::gateway::{GatewaySpec, Listener};
    use http::{Response, StatusCode};
    use kube::client::Body;
    use serde_json::json;
    use std::convert::Infallible;
    use tower::service_fn;

    struct NoopGatewayHandler;

    #[async_trait::async_trait]
    impl ProcessorHandler<Gateway> for NoopGatewayHandler {}

    struct NoopAcmeHandler;

    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionAcme> for NoopAcmeHandler {}

    fn test_gateway(proof_epoch: &str) -> Gateway {
        Gateway {
            metadata: kube::api::ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("gw".to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: GatewaySpec {
                current_status: Some(
                    serde_json::from_value(serde_json::json!({
                        "listeners": [{"name":"http", "supportedKinds":[], "attachedRoutes":0, "conditions":[]}]
                    }))
                    .unwrap(),
                ),
                gateway_class_name: "edgion".to_string(),
                listeners: Some(vec![Listener {
                    name: "http".to_string(),
                    hostname: None,
                    port: 80,
                    protocol: "HTTP".to_string(),
                    tls: None,
                    allowed_routes: None,
                }]),
                addresses: None,
                tls: None,
                allowed_listeners: None,
                infrastructure: None,
                default_scope: None,
                resolved_inbound_proxy_protocol: Default::default(),
                resolved_attachment_proof: Some(GatewayAttachmentProof {
                    instance: "test-instance".to_string(),
                    generation: 1,
                    epoch: proof_epoch.to_string(),
                    attachment_digest: "a".repeat(64),
                }),
            },
            status: None,
        }
    }

    fn test_acme() -> EdgionAcme {
        serde_yaml::from_str(
            r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  namespace: default
  name: test
  uid: uid-1
  generation: 1
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: account
  domains:
    - example.com
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: certificate
"#,
        )
        .expect("test EdgionAcme must deserialize")
    }

    fn eligibility_runtime(processed: &mut EdgionAcme) -> Arc<ControllerRuntime> {
        let runtime = Arc::new(ControllerRuntime::default());
        let gateway_processor = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            8,
            Arc::new(NoopGatewayHandler),
            runtime.clone(),
        ));
        gateway_processor.save(test_gateway("1"), None);
        runtime.processor_registry.register(gateway_processor);

        let gateway_ref = match &processed.spec.challenge {
            AcmeChallenge::Http01 { gateway_ref } => gateway_ref,
        };
        processed.spec.resolved_listener_attachments = Some(
            resolve_acme_listener_attachments(gateway_ref, "default", &runtime)
                .expect("test Gateway attachment must resolve"),
        );
        let generation = processed.metadata.generation;
        let status = processed.spec.current_status.get_or_insert_with(Default::default);
        status.conditions = vec![
            condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Resource accepted",
                generation,
            ),
            condition_true(
                condition_types::RESOLVED_REFS,
                condition_reasons::RESOLVED_REFS,
                "References resolved",
                generation,
            ),
        ];
        let acme_processor = Arc::new(ResourceProcessor::<EdgionAcme>::new(
            "EdgionAcme",
            8,
            Arc::new(NoopAcmeHandler),
            runtime.clone(),
        ));
        acme_processor.save(processed.clone(), None);
        runtime.processor_registry.register(acme_processor);
        runtime
    }

    fn test_attempt(client: Client, acme: EdgionAcme, runtime: Arc<ControllerRuntime>, generation: u64) -> AcmeAttempt {
        let serving_attachments = admitted_serving_attachments("default/test", &runtime).unwrap_or_default();
        AcmeAttempt {
            attempt_id: "test-attempt".to_string(),
            client,
            acme,
            runtime,
            leader: None,
            leader_fence: None,
            serving_attachments,
            certificate_issued: Arc::new(AtomicBool::new(false)),
            generation,
            cancel: CancellationToken::new(),
        }
    }

    fn empty_acme_list() -> Response<Body> {
        Response::builder()
            .status(StatusCode::OK)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "apiVersion": "edgion.io/v1",
                    "kind": "EdgionAcmeList",
                    "metadata": {"resourceVersion": "1"},
                    "items": []
                }))
                .expect("response JSON must serialize"),
            ))
            .expect("response must build")
    }

    #[tokio::test]
    async fn certificate_publication_rechecks_authority_between_writes() {
        let _guard = ACME_TEST_MUTEX.lock().await;
        // Simulate a completed CA request, then change the resource before
        // publication or between its writes. No live CA or sleeps are needed.
        for (scenario, expected_paths) in [
            (
                "proof-feedback",
                vec![
                    "/api/v1/namespaces/default/secrets/certificate",
                    "/apis/edgion.io/v1/namespaces/default/edgiontls/acme-test",
                    "/apis/edgion.io/v1/namespaces/default/edgionacmes/test/status",
                ],
            ),
            (
                "current",
                vec![
                    "/api/v1/namespaces/default/secrets/certificate",
                    "/apis/edgion.io/v1/namespaces/default/edgiontls/acme-test",
                    "/apis/edgion.io/v1/namespaces/default/edgionacmes/test/status",
                ],
            ),
            ("uid", vec![]),
            ("generation", vec![]),
            ("deleted", vec![]),
            ("get-error", vec![]),
            ("stopped", vec![]),
            ("stop-during-get", vec![]),
            ("ownership", vec![]),
            ("after-secret", vec!["/api/v1/namespaces/default/secrets/certificate"]),
            (
                "after-tls",
                vec![
                    "/api/v1/namespaces/default/secrets/certificate",
                    "/apis/edgion.io/v1/namespaces/default/edgiontls/acme-test",
                ],
            ),
        ] {
            let capture = NotificationCapture::install();
            let mut acme = test_acme();
            acme.metadata.resource_version = Some("10".into());
            acme.spec.auto_edgion_tls.enabled = true;
            let runtime = eligibility_runtime(&mut acme.clone());
            if scenario == "ownership" {
                runtime
                    .processor_registry
                    .register(Arc::new(ResourceProcessor::<Gateway>::new(
                        "Gateway",
                        8,
                        Arc::new(NoopGatewayHandler),
                        runtime.clone(),
                    )));
            }
            if scenario == "stopped" {
                ACME_SERVICE.lock().take();
            }
            let writes = Arc::new(Mutex::new(Vec::new()));
            let recorded = writes.clone();
            let mut current = acme.clone();
            // A status-only resourceVersion change does not retire an attempt.
            current.metadata.resource_version = Some("11".into());
            if scenario == "uid" {
                current.metadata.uid = Some("replacement".into());
            }
            if scenario == "generation" {
                current.metadata.generation = Some(2);
            }
            let feedback_runtime = runtime.clone();
            let client = Client::new(
                service_fn(move |request: http::Request<Body>| {
                    let recorded = recorded.clone();
                    let mut current = current.clone();
                    let feedback_runtime = feedback_runtime.clone();
                    async move {
                        let (parts, body) = request.into_parts();
                        let value = if parts.method == http::Method::GET && parts.uri.path().contains("/edgionacmes/") {
                            if scenario == "stop-during-get" {
                                ACME_SERVICE.lock().take();
                            }
                            if scenario == "deleted" || scenario == "get-error" {
                                let code = if scenario == "deleted" { 404 } else { 503 };
                                return Ok::<_, Infallible>(
                                    Response::builder()
                                        .status(code)
                                        .header(http::header::CONTENT_TYPE, "application/json")
                                        .body(Body::from(
                                            json!({"apiVersion":"v1", "kind":"Status", "status":"Failure",
                                    "message":"test read failure", "reason":"Unavailable", "code":code})
                                            .to_string()
                                            .into_bytes(),
                                        ))
                                        .unwrap(),
                                );
                            }
                            let count = recorded.lock().len();
                            if (scenario == "after-secret" && count >= 1) || (scenario == "after-tls" && count >= 2) {
                                current.metadata.generation = Some(2);
                            }
                            serde_json::to_value(current).unwrap()
                        } else if parts.method == http::Method::GET {
                            return Ok::<_, Infallible>(
                                Response::builder()
                                    .status(404)
                                    .header(http::header::CONTENT_TYPE, "application/json")
                                    .body(Body::from(
                                        json!({"apiVersion":"v1", "kind":"Status", "status":"Failure",
                                            "message":"not found", "reason":"NotFound", "code":404})
                                        .to_string()
                                        .into_bytes(),
                                    ))
                                    .unwrap(),
                            );
                        } else {
                            assert_eq!(parts.method, http::Method::PATCH);
                            let value: serde_json::Value =
                                serde_json::from_slice(&body.collect_bytes().await.unwrap()).unwrap();
                            recorded.lock().push((parts.uri.path().to_string(), value.clone()));
                            if scenario == "proof-feedback" && parts.uri.path().ends_with("/secrets/certificate") {
                                refresh_test_serving_proof(&feedback_runtime);
                            }
                            if parts.uri.path().ends_with("/status") {
                                let mut response = serde_json::to_value(current).unwrap();
                                response["status"] = value["status"].clone();
                                response
                            } else {
                                value
                            }
                        };
                        Ok::<_, Infallible>(
                            Response::builder()
                                .header(http::header::CONTENT_TYPE, "application/json")
                                .body(Body::from(value.to_string().into_bytes()))
                                .unwrap(),
                        )
                    }
                }),
                "default",
            );
            let api = Api::namespaced(client.clone(), "default");
            let attempt = test_attempt(client.clone(), acme, runtime, capture.generation);
            attempt.certificate_issued.store(true, Ordering::Release);
            let result = publish_certificate(
                &client,
                &api,
                &attempt,
                &AcmeCertificateResult {
                    certificate_pem: "test certificate".into(),
                    private_key_pem: Zeroizing::new("test key".into()),
                },
                "test-account",
            )
            .await;
            assert_eq!(
                result.is_ok(),
                matches!(scenario, "current" | "proof-feedback"),
                "{scenario}: {result:?}"
            );
            let writes = writes.lock();
            let paths: Vec<_> = writes.iter().map(|(path, _)| path.as_str()).collect();
            assert_eq!(paths, expected_paths, "{scenario}");
            if scenario == "current" {
                assert_eq!(writes[2].1["metadata"]["uid"], "uid-1");
                assert_eq!(writes[2].1["metadata"]["resourceVersion"], "11");
                assert_eq!(writes[2].1["status"]["phase"], "Ready");
            }
        }
    }

    fn refresh_test_serving_proof(runtime: &Arc<ControllerRuntime>) {
        let gateway = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            8,
            Arc::new(NoopGatewayHandler),
            runtime.clone(),
        ));
        let mut refreshed = test_gateway("certificate-reload");
        refreshed
            .spec
            .resolved_attachment_proof
            .as_mut()
            .unwrap()
            .attachment_digest = "b".repeat(64);
        gateway.save(refreshed, None);
        runtime.processor_registry.register(gateway);
        let mut processed = runtime
            .processor_registry
            .get("EdgionAcme")
            .unwrap()
            .get_cached_any("default/test")
            .unwrap()
            .downcast::<EdgionAcme>()
            .unwrap()
            .as_ref()
            .clone();
        let AcmeChallenge::Http01 { gateway_ref } = &processed.spec.challenge;
        processed.spec.resolved_listener_attachments =
            Some(resolve_acme_listener_attachments(gateway_ref, "default", runtime).unwrap());
        let processor = Arc::new(ResourceProcessor::<EdgionAcme>::new(
            "EdgionAcme",
            8,
            Arc::new(NoopAcmeHandler),
            runtime.clone(),
        ));
        processor.save(processed, None);
        runtime.processor_registry.register(processor);
    }

    #[tokio::test(start_paused = true)]
    async fn operation_guards_retain_local_state_through_transient_reads() {
        let _lock = ACME_TEST_MUTEX.lock().await;
        let capture = NotificationCapture::install();
        let acme = test_acme();
        let runtime = eligibility_runtime(&mut acme.clone());
        let fail_next = Arc::new(AtomicBool::new(false));
        let response_fail = fail_next.clone();
        let response_acme = acme.clone();
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                assert_eq!(request.method(), http::Method::GET);
                let fail = response_fail.swap(false, Ordering::SeqCst);
                let body = serde_json::to_value(&response_acme).unwrap();
                async move {
                    Ok::<_, Infallible>(test_api_response(
                        if fail { 503 } else { 200 },
                        if fail {
                            json!({"apiVersion":"v1", "kind":"Status", "status":"Failure", "reason":"Unavailable", "code":503})
                        } else {
                            body
                        },
                    ))
                }
            }),
            "default",
        );
        let attempt = test_attempt(client, acme, runtime, capture.generation);
        let mut completed_operations = Vec::new();
        for operation in [
            "activate HTTP-01 challenge",
            "finalize ACME order",
            "persist ACME account credentials",
        ] {
            fail_next.store(true, Ordering::SeqCst);
            let before = Instant::now();
            attempt.check(operation).await.unwrap();
            assert!(Instant::now().duration_since(before) >= CONTROL_PLANE_RECHECK_DELAY);
            completed_operations.push(operation);
        }
        assert_eq!(completed_operations.len(), 3);
        fail_next.store(true, Ordering::SeqCst);
        let check = attempt.check("finalize ACME order");
        tokio::pin!(check);
        tokio::select! { result = &mut check => panic!("guard must await uncertainty: {result:?}"), _ = tokio::task::yield_now() => {} }
        attempt.cancel.cancel();
        assert!(check.await.is_err());
    }

    fn test_api_response(code: u16, value: serde_json::Value) -> Response<Body> {
        Response::builder()
            .status(code)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string().into_bytes()))
            .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn account_secret_conflict_adopts_concurrent_credentials_or_retries_empty_snapshot() {
        let _lock = ACME_TEST_MUTEX.lock().await;
        let capture = NotificationCapture::install();
        for scenario in ["credentials", "empty", "replaced", "corrupt"] {
            let acme = test_acme();
            let runtime = eligibility_runtime(&mut acme.clone());
            let snapshot: Secret = serde_json::from_value(json!({"apiVersion":"v1", "kind":"Secret",
                "metadata":{"name":"account", "namespace":"default", "uid":"account-uid", "resourceVersion":"10"}}))
            .unwrap();
            // Deserialization is the production validation boundary here;
            // account restoration itself remains the ACME client's operation.
            let winner = json!({"id":"https://ca.example/account/winner", "key_pkcs8":"AQID", "directory":"https://ca.example/directory"}).to_string();
            let mut refreshed = snapshot.clone();
            refreshed.metadata.resource_version = Some("11".into());
            if scenario == "replaced" {
                refreshed.metadata.uid = Some("replacement-uid".into());
            }
            if matches!(scenario, "credentials" | "corrupt") {
                refreshed.data = Some(BTreeMap::from([(
                    edgion_resources::credentials::ACME_ACCOUNT_KEY.to_string(),
                    ByteString(if scenario == "credentials" {
                        winner.as_bytes().to_vec()
                    } else {
                        b"invalid".to_vec()
                    }),
                )]));
            }
            let patches = Arc::new(Mutex::new(Vec::new()));
            let recorded = patches.clone();
            let response_acme = acme.clone();
            let client = Client::new(
                service_fn(move |request: http::Request<Body>| {
                    let recorded = recorded.clone();
                    let refreshed = refreshed.clone();
                    let response_acme = response_acme.clone();
                    async move {
                        let (parts, body) = request.into_parts();
                        if parts.uri.path().contains("/edgionacmes/") {
                            assert_eq!(parts.method, http::Method::GET);
                            return Ok::<_, Infallible>(test_api_response(
                                200,
                                serde_json::to_value(response_acme).unwrap(),
                            ));
                        }
                        if parts.method == http::Method::PATCH {
                            let patch: serde_json::Value =
                                serde_json::from_slice(&body.collect_bytes().await.unwrap()).unwrap();
                            assert_eq!(patch["metadata"]["uid"], "account-uid");
                            let mut patches = recorded.lock();
                            assert_eq!(
                                patch["metadata"]["resourceVersion"],
                                if patches.is_empty() { "10" } else { "11" }
                            );
                            patches.push(patch);
                            if patches.len() == 1 {
                                return Ok(test_api_response(
                                    409,
                                    json!({"apiVersion":"v1", "kind":"Status", "status":"Failure", "reason":"Conflict", "code":409}),
                                ));
                            }
                        }
                        Ok(test_api_response(200, serde_json::to_value(refreshed).unwrap()))
                    }
                }),
                "default",
            );
            let api = Api::namespaced(client.clone(), "default");
            let attempt = test_attempt(client, acme, runtime, capture.generation);
            let result = persist_registered_account(
                &attempt,
                &api,
                "account",
                snapshot,
                &Zeroizing::new("newly registered credentials".into()),
            )
            .await;
            match scenario {
                "credentials" => {
                    let adopted = result.unwrap().unwrap();
                    assert_eq!(
                        serde_json::to_value(adopted).unwrap()["id"],
                        "https://ca.example/account/winner"
                    );
                    assert_eq!(patches.lock().len(), 1);
                }
                "empty" => {
                    assert!(result.unwrap().is_none());
                    assert_eq!(patches.lock().len(), 2);
                }
                _ => {
                    assert!(result.is_err());
                    assert_eq!(patches.lock().len(), 1);
                }
            }
        }
    }

    #[tokio::test]
    async fn resource_notification_preserves_issued_certificate_but_cancels_preissuance() {
        let _lock = ACME_TEST_MUTEX.lock().await;
        let capture = NotificationCapture::install();
        let acme = test_acme();
        let runtime = eligibility_runtime(&mut acme.clone());
        let client = Client::new(
            service_fn(|_: http::Request<Body>| async { Ok::<_, Infallible>(test_api_response(500, json!({}))) }),
            "default",
        );
        let mut worker = AcmeServiceWorker {
            client,
            runtime,
            leader: None,
            generation: capture.generation,
            cancel: CancellationToken::new(),
            schedules: HashMap::new(),
            jobs: FuturesUnordered::new(),
        };
        for issued in [false, true] {
            let cancel = CancellationToken::new();
            worker.schedules.insert(
                "default/test".into(),
                ResourceSchedule {
                    busy: true,
                    active_cancel: Some(cancel.clone()),
                    certificate_issued: Some(Arc::new(AtomicBool::new(issued))),
                    ..Default::default()
                },
            );
            worker.enqueue_check("default/test".into(), TriggerKind::Resource);
            assert_eq!(cancel.is_cancelled(), !issued);
            assert_eq!(worker.schedules["default/test"].pending, Some(TriggerKind::Resource));
            assert!(worker.jobs.is_empty());
        }
    }

    #[tokio::test]
    async fn exhausted_interrupted_attempt_recovers_failed_without_reservation() {
        let _lock = ACME_TEST_MUTEX.lock().await;
        let capture = NotificationCapture::install();
        for phase in [AcmeCertPhase::Issuing, AcmeCertPhase::Renewing] {
            let mut acme = test_acme();
            acme.metadata.resource_version = Some("10".into());
            acme.status = Some(EdgionAcmeStatus {
                phase,
                scheduler: Some(AcmeSchedulerStatus {
                    observed_generation: 1,
                    attempts_used: MAX_RETRY_ATTEMPTS,
                    attempt_id: Some("interrupted".into()),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let runtime = eligibility_runtime(&mut acme.clone());
            let patches = Arc::new(Mutex::new(Vec::new()));
            let fail_write = Arc::new(AtomicBool::new(true));
            let recorded = patches.clone();
            let response_fail = fail_write.clone();
            let client = Client::new(
                service_fn(move |request: http::Request<Body>| {
                    let acme = acme.clone();
                    let recorded = recorded.clone();
                    let response_fail = response_fail.clone();
                    async move {
                        let (parts, body) = request.into_parts();
                        if parts.method == http::Method::PATCH {
                            let patch: serde_json::Value =
                                serde_json::from_slice(&body.collect_bytes().await.unwrap()).unwrap();
                            assert_eq!(patch["status"]["phase"], "Failed");
                            assert!(patch["status"].get("scheduler").is_none());
                            assert_eq!(patch["metadata"]["uid"], "uid-1");
                            recorded.lock().push(patch);
                            if response_fail.swap(false, Ordering::SeqCst) {
                                return Ok::<_, Infallible>(test_api_response(
                                    503,
                                    json!({"kind":"Status", "apiVersion":"v1", "status":"Failure", "reason":"Unavailable", "code":503}),
                                ));
                            }
                        }
                        Ok::<_, Infallible>(test_api_response(200, serde_json::to_value(acme).unwrap()))
                    }
                }),
                "default",
            );
            for expected_retry in [true, false] {
                let outcome = check_resource(
                    client.clone(),
                    runtime.clone(),
                    None,
                    capture.generation,
                    CancellationToken::new(),
                    "default/test",
                    TriggerKind::Startup,
                )
                .await;
                assert_eq!(matches!(outcome, CheckOutcome::Recheck(_, _)), expected_retry);
                if !expected_retry {
                    assert!(matches!(outcome, CheckOutcome::Idle));
                }
            }
            assert_eq!(patches.lock().len(), 2);
        }
    }

    #[tokio::test]
    async fn retired_attempt_cannot_publish_challenges_or_failure() {
        let _guard = ACME_TEST_MUTEX.lock().await;
        let capture = NotificationCapture::install();
        let acme = test_acme();
        let runtime = eligibility_runtime(&mut acme.clone());
        let mut replacement = acme.clone();
        replacement.metadata.uid = Some("replacement".into());
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                assert_eq!(request.method(), http::Method::GET, "retired attempt must not write");
                let body = serde_json::to_vec(&replacement).unwrap();
                async move {
                    Ok::<_, Infallible>(
                        Response::builder()
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Body::from(body))
                            .unwrap(),
                    )
                }
            }),
            "default",
        );
        let api = Api::namespaced(client.clone(), "default");
        let attempt = test_attempt(client, acme, runtime, capture.generation);
        let challenges = [ActiveHttpChallenge {
            domain: "example.com".into(),
            token: "test-token".into(),
            key_authorization: "test-authorization".into(),
            expire_at: 600,
        }];
        assert!(patch_active_challenges(&api, &attempt, "test", Some(&challenges))
            .await
            .is_err());
        assert!(patch_active_challenges(&api, &attempt, "test", None).await.is_err());
        assert!(
            update_acme_status(&api, &attempt, "test", AcmeCertPhase::Failed, |_| {})
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn admitted_attempt_rejects_a_new_serving_proof() {
        let _guard = ACME_TEST_MUTEX.lock().await;
        let capture = NotificationCapture::install();
        let acme = test_acme();
        let mut processed = acme.clone();
        let runtime = eligibility_runtime(&mut processed);
        let client = Client::new(
            service_fn({
                let acme = acme.clone();
                move |_request: http::Request<Body>| {
                    let body = serde_json::to_vec(&acme).unwrap();
                    async move {
                        Ok::<_, Infallible>(
                            Response::builder()
                                .header(http::header::CONTENT_TYPE, "application/json")
                                .body(Body::from(body))
                                .unwrap(),
                        )
                    }
                }
            }),
            "default",
        );
        let attempt = test_attempt(client.clone(), acme.clone(), runtime.clone(), capture.generation);

        let replacement_gateway = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            8,
            Arc::new(NoopGatewayHandler),
            runtime.clone(),
        ));
        replacement_gateway.save(test_gateway("2"), None);
        runtime.processor_registry.register(replacement_gateway);
        let gateway_ref = match &processed.spec.challenge {
            AcmeChallenge::Http01 { gateway_ref } => gateway_ref,
        };
        processed.spec.resolved_listener_attachments = Some(
            resolve_acme_listener_attachments(gateway_ref, "default", &runtime)
                .expect("replacement proof must resolve"),
        );
        let replacement_acme = Arc::new(ResourceProcessor::<EdgionAcme>::new(
            "EdgionAcme",
            8,
            Arc::new(NoopAcmeHandler),
            runtime.clone(),
        ));
        replacement_acme.save(processed, None);
        runtime.processor_registry.register(replacement_acme);

        let api = Api::namespaced(client, "default");
        assert!(matches!(
            attempt.current_resource_typed(&api).await,
            Err(AttemptAuthorityError::Lost(_))
        ));
        attempt.certificate_issued.store(true, Ordering::Release);
        assert!(attempt.current_resource_typed(&api).await.is_ok());
        let current = admitted_serving_attachments("default/test", &runtime).unwrap();
        for change in ["gateway-uid", "gateway-generation", "listener", "selection"] {
            let mut changed = current.clone();
            match change {
                "gateway-uid" => changed[0].gateway_proof.instance = "replacement".into(),
                "gateway-generation" => changed[0].gateway_proof.generation += 1,
                "listener" => changed[0].listener_name = "other".into(),
                _ => changed.clear(),
            }
            assert!(
                !same_serving_selection(&attempt.serving_attachments, &changed),
                "{change}"
            );
        }
    }

    #[tokio::test]
    async fn ready_recovery_clears_stale_scheduler_checkpoint() {
        let _guard = ACME_TEST_MUTEX.lock().await;
        let capture = NotificationCapture::install();
        let mut acme = test_acme();
        acme.metadata.resource_version = Some("10".into());
        let runtime = eligibility_runtime(&mut acme.clone());
        acme.status = Some(EdgionAcmeStatus {
            phase: AcmeCertPhase::Ready,
            certificate_not_after: Some("2099-01-01T00:00:00Z".into()),
            scheduler: Some(AcmeSchedulerStatus {
                observed_generation: 1,
                attempt_id: Some("completed-attempt".into()),
                attempts_used: 4,
                attempt_started_at: Some("2026-01-01T00:00:00Z".into()),
                next_retry_at: Some("2098-01-01T00:00:00Z".into()),
            }),
            ..Default::default()
        });
        let patches = Arc::new(Mutex::new(Vec::new()));
        let recorded = patches.clone();
        let response_acme = acme.clone();
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                let recorded = recorded.clone();
                let response_acme = response_acme.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    if parts.method == http::Method::PATCH {
                        let bytes = body.collect_bytes().await.unwrap();
                        let patch = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
                        recorded.lock().push(patch);
                    }
                    Ok::<_, Infallible>(
                        Response::builder()
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Body::from(serde_json::to_vec(&response_acme).unwrap()))
                            .unwrap(),
                    )
                }
            }),
            "default",
        );
        let outcome = check_resource(
            client,
            runtime,
            None,
            capture.generation,
            CancellationToken::new(),
            "default/test",
            TriggerKind::Startup,
        )
        .await;
        assert!(matches!(outcome, CheckOutcome::Wait(_, TriggerKind::Renewal)));
        let patches = patches.lock();
        assert_eq!(patches.len(), 1);
        assert!(patches[0]["status"]["scheduler"].is_null());
    }

    #[tokio::test]
    async fn terminating_resource_does_not_start_an_attempt() {
        let _guard = ACME_TEST_MUTEX.lock().await;
        let capture = NotificationCapture::install();
        let mut acme = test_acme();
        acme.metadata.deletion_timestamp =
            Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(chrono::Utc::now()));
        let runtime = eligibility_runtime(&mut acme.clone());
        let reads = Arc::new(AtomicU64::new(0));
        let read_count = reads.clone();
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                assert_eq!(request.method(), http::Method::GET);
                read_count.fetch_add(1, Ordering::SeqCst);
                let body = serde_json::to_vec(&acme).unwrap();
                async move {
                    Ok::<_, Infallible>(
                        Response::builder()
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Body::from(body))
                            .unwrap(),
                    )
                }
            }),
            "default",
        );
        let outcome = check_resource(
            client,
            runtime,
            None,
            capture.generation,
            CancellationToken::new(),
            "default/test",
            TriggerKind::Resource,
        )
        .await;
        assert!(matches!(outcome, CheckOutcome::Deleted));
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn resource_get_classifies_not_found_and_transient_failure() {
        for (code, deleted) in [(404, true), (503, false)] {
            let client = Client::new(
                service_fn(move |_request: http::Request<Body>| async move {
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(code)
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                json!({
                                    "apiVersion": "v1",
                                    "kind": "Status",
                                    "status": "Failure",
                                    "message": "test read failure",
                                    "reason": if code == 404 { "NotFound" } else { "Unavailable" },
                                    "code": code,
                                })
                                .to_string()
                                .into_bytes(),
                            ))
                            .unwrap(),
                    )
                }),
                "default",
            );
            let trigger = if code == 503 {
                TriggerKind::Admin
            } else {
                TriggerKind::Startup
            };
            let outcome = check_resource(
                client,
                Arc::new(ControllerRuntime::default()),
                None,
                1,
                CancellationToken::new(),
                "default/test",
                trigger,
            )
            .await;
            assert_eq!(matches!(&outcome, CheckOutcome::Deleted), deleted, "HTTP {code}");
            assert_eq!(
                matches!(&outcome, CheckOutcome::Recheck(delay, retry_trigger)
                    if *delay == CONTROL_PLANE_RECHECK_DELAY && *retry_trigger == trigger),
                !deleted,
                "HTTP {code}"
            );
        }
    }

    #[tokio::test]
    async fn startup_scan_lists_only_configured_namespaces() {
        let paths = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&paths);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                captured.lock().push(request.uri().path().to_string());
                Ok::<_, Infallible>(empty_acme_list())
            }
        });

        let client = Client::new(service, "default");
        let items = list_acme_resources(
            client,
            &["team-a".into(), "team-b".into()],
            &kube::api::ListParams::default(),
        )
        .await
        .expect("namespace-scoped lists must succeed");

        assert!(items.is_empty());
        assert_eq!(
            *paths.lock(),
            vec![
                "/apis/edgion.io/v1/namespaces/team-a/edgionacmes",
                "/apis/edgion.io/v1/namespaces/team-b/edgionacmes"
            ]
        );
    }

    // ================================================================
    // exponential_backoff
    // ================================================================

    #[test]
    fn test_exponential_backoff_sequence() {
        // base=300s, multiplier=4^attempt
        assert_eq!(exponential_backoff(300, 0), Duration::from_secs(300)); // 5min
        assert_eq!(exponential_backoff(300, 1), Duration::from_secs(1200)); // 20min
        assert_eq!(exponential_backoff(300, 2), Duration::from_secs(4800)); // 80min
        assert_eq!(exponential_backoff(300, 3), Duration::from_secs(19200)); // 5.3h
        assert_eq!(exponential_backoff(300, 4), Duration::from_secs(76800)); // 21.3h
    }

    #[test]
    fn test_exponential_backoff_saturates() {
        // Very large attempt should not overflow (saturating_pow + saturating_mul)
        let result = exponential_backoff(300, 100);
        assert!(result.as_secs() > 0); // Should not be zero/wrap
    }

    #[test]
    fn issuance_eligibility_missing_owner_does_not_start_transient_retry() {
        use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::KubernetesStatusIdentity;
        let raw = test_acme();
        let runtime = ControllerRuntime::default();
        runtime.set_status_identity(KubernetesStatusIdentity {
            controller_name: "example.com/controller".into(),
        });
        assert_eq!(
            evaluate_issuance_eligibility(&raw, "default/test-acme", &runtime),
            EligibilityDecision::AuthoritativelyIneligible
        );
    }

    #[tokio::test]
    async fn issuance_eligibility_requires_matching_processed_generation_and_live_proof() {
        let raw = test_acme();
        let mut processed = raw.clone();
        let runtime = eligibility_runtime(&mut processed);
        assert_eq!(
            evaluate_issuance_eligibility(&raw, "default/test", &runtime),
            EligibilityDecision::Eligible
        );

        let mut replaced_raw = raw.clone();
        replaced_raw.metadata.uid = Some("uid-2".to_string());
        assert_eq!(
            evaluate_issuance_eligibility(&replaced_raw, "default/test", &runtime),
            EligibilityDecision::Transient
        );

        let replacement = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            8,
            Arc::new(NoopGatewayHandler),
            runtime.clone(),
        ));
        replacement.save(test_gateway("2"), None);
        runtime.processor_registry.register(replacement);
        assert_eq!(
            evaluate_issuance_eligibility(&raw, "default/test", &runtime),
            EligibilityDecision::Transient,
            "a proof-only Gateway publication must invalidate stale processed authority"
        );

        let missing = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            8,
            Arc::new(NoopGatewayHandler),
            runtime.clone(),
        ));
        runtime.processor_registry.register(missing);
        assert_eq!(
            evaluate_issuance_eligibility(&raw, "default/test", &runtime),
            EligibilityDecision::Transient,
            "a projected-Gateway publication gap must retain a convergence check"
        );

        let mut no_http_gateway = test_gateway("3");
        let listener = no_http_gateway
            .spec
            .listeners
            .as_mut()
            .and_then(|listeners| listeners.first_mut())
            .expect("test Gateway listener must exist");
        listener.protocol = "HTTPS".to_string();
        listener.port = 443;
        let no_http = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            8,
            Arc::new(NoopGatewayHandler),
            runtime.clone(),
        ));
        no_http.save(no_http_gateway, None);
        runtime.processor_registry.register(no_http);
        assert_eq!(
            evaluate_issuance_eligibility(&raw, "default/test", &runtime),
            EligibilityDecision::AuthoritativelyIneligible,
            "a live non-HTTP selection is an authoritative invalid result"
        );
    }

    #[tokio::test]
    async fn issuance_eligibility_rejects_current_false_condition() {
        let raw = test_acme();
        let mut processed = raw.clone();
        let runtime = eligibility_runtime(&mut processed);
        let status = processed.spec.current_status.get_or_insert_with(Default::default);
        status
            .conditions
            .retain(|condition| condition.type_ != condition_types::RESOLVED_REFS);
        status.conditions.push(condition_false(
            condition_types::RESOLVED_REFS,
            condition_reasons::NO_MATCHING_PARENT,
            "No HTTP listener",
            processed.metadata.generation,
        ));
        let replacement = Arc::new(ResourceProcessor::<EdgionAcme>::new(
            "EdgionAcme",
            8,
            Arc::new(NoopAcmeHandler),
            runtime.clone(),
        ));
        replacement.save(processed, None);
        runtime.processor_registry.register(replacement);
        assert_eq!(
            evaluate_issuance_eligibility(&raw, "default/test", &runtime),
            EligibilityDecision::AuthoritativelyIneligible
        );
    }

    // ================================================================
    // parse_resource_key
    // ================================================================

    #[test]
    fn test_parse_resource_key_normal() {
        let (ns, name) = parse_resource_key("default/my-acme");
        assert_eq!(ns, "default");
        assert_eq!(name, "my-acme");
    }

    #[test]
    fn test_parse_resource_key_with_slash_in_name() {
        // splitn(2, '/') means only split on the first '/'
        let (ns, name) = parse_resource_key("ns/name/with/slashes");
        assert_eq!(ns, "ns");
        assert_eq!(name, "name/with/slashes");
    }

    #[test]
    fn test_parse_resource_key_no_namespace() {
        let (ns, name) = parse_resource_key("just-a-name");
        assert_eq!(ns, "default");
        assert_eq!(name, "just-a-name");
    }

    // ================================================================
    // parse_cert_info
    // ================================================================

    // ================================================================
    // timer generation isolation
    // ================================================================

    /// A timer scheduled by one ACME service generation must not enqueue into a
    /// later generation after a stop/restart. All assertions live in a single
    /// test because they mutate the `ACME_SERVICE` global; splitting them would
    /// let the cases race against each other.
    #[tokio::test]
    async fn test_service_generation_sender_isolation() {
        // Serialize against any other test that touches the global `ACME_SERVICE`
        // (e.g. the handler tests whose `after_publish` reaches `notify_resource_changed`
        // -> `get_sender()`), so a concurrent notify cannot clone the handle we
        // install below and enqueue a stray message into our receiver.
        let _guard = ACME_TEST_MUTEX.lock().await;

        // --- Generation 1 starts ---
        let (tx1, _rx1) = mpsc::channel::<AcmeCommand>(8);
        let gen1 = ACME_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        *ACME_SERVICE.lock() = Some(AcmeServiceHandle {
            tx: tx1,
            generation: gen1,
            cancel: CancellationToken::new(),
        });

        // A timer stamped with gen1 can enqueue while gen1 is running.
        assert!(get_sender_for_generation(gen1).is_some());
        assert_eq!(current_generation(), gen1);

        // --- Service stops (drops the handle) ---
        ACME_SERVICE.lock().take();
        assert!(get_sender_for_generation(gen1).is_none());
        assert_eq!(current_generation(), 0);

        // --- Generation 2 starts (re-election) ---
        let (tx2, mut rx2) = mpsc::channel::<AcmeCommand>(8);
        let gen2 = ACME_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(gen2 > gen1, "generations must be strictly increasing");
        *ACME_SERVICE.lock() = Some(AcmeServiceHandle {
            tx: tx2,
            generation: gen2,
            cancel: CancellationToken::new(),
        });

        // A stale gen1 timer must NOT see gen2's sender.
        assert!(get_sender_for_generation(gen1).is_none());
        // The current generation's timer still works.
        assert!(get_sender_for_generation(gen2).is_some());

        assert!(get_sender_for_generation(gen1).is_none());
        get_sender_for_generation(gen2)
            .unwrap()
            .send(AcmeCommand::StartupCheck {
                key: "default/live".to_string(),
            })
            .await
            .unwrap();
        let live = rx2.recv().await;
        assert!(matches!(live, Some(AcmeCommand::StartupCheck { key }) if key == "default/live"));

        // Cleanup global state.
        ACME_SERVICE.lock().take();
    }

    #[test]
    fn scheduler_keeps_one_earliest_deadline_and_admin_pending() {
        let mut state = ResourceSchedule::default();
        state.set_deadline(Duration::from_secs(30), TriggerKind::Renewal);
        let first = state.deadline.unwrap().0;
        state.set_deadline(Duration::from_secs(60), TriggerKind::Retry);
        assert_eq!(state.deadline.unwrap().0, first);
        state.set_deadline(Duration::from_secs(1), TriggerKind::Eligibility);
        assert!(state.deadline.unwrap().0 < first);

        state.merge_pending(TriggerKind::Resource);
        state.merge_pending(TriggerKind::Admin);
        state.merge_pending(TriggerKind::Startup);
        assert_eq!(state.pending, Some(TriggerKind::Admin));
    }

    #[test]
    fn admin_recheck_priority_survives_ordinary_notifications() {
        let mut waiting = ResourceSchedule::default();
        waiting.set_deadline(CONTROL_PLANE_RECHECK_DELAY, TriggerKind::Admin);
        assert_eq!(claim_trigger(&mut waiting, TriggerKind::Resource), TriggerKind::Admin);
        assert!(waiting.deadline.is_none());

        let mut completing = ResourceSchedule {
            busy: true,
            pending: Some(TriggerKind::Resource),
            ..Default::default()
        };
        completing.set_deadline(CONTROL_PLANE_RECHECK_DELAY, TriggerKind::Admin);
        completing.busy = false;
        let pending = completing.pending.take().unwrap();
        assert_eq!(claim_trigger(&mut completing, pending), TriggerKind::Admin);
    }

    #[test]
    fn delete_completion_preserves_a_concurrent_recreate_notification() {
        let mut schedules = HashMap::from([(
            "default/test".to_string(),
            ResourceSchedule {
                busy: true,
                pending: Some(TriggerKind::Resource),
                ..Default::default()
            },
        )]);
        assert_eq!(
            take_deleted_pending(&mut schedules, "default/test"),
            Some(TriggerKind::Resource)
        );
        assert!(!schedules.contains_key("default/test"));
    }

    #[test]
    fn persisted_retry_budget_resets_only_for_admin_or_new_generation() {
        let checkpoint = AcmeSchedulerStatus {
            observed_generation: 7,
            attempts_used: 4,
            ..Default::default()
        };
        assert_eq!(persisted_attempts(TriggerKind::Resource, 7, Some(&checkpoint)), 4);
        assert_eq!(persisted_attempts(TriggerKind::Startup, 7, Some(&checkpoint)), 4);
        assert_eq!(persisted_attempts(TriggerKind::Admin, 7, Some(&checkpoint)), 0);
        assert_eq!(persisted_attempts(TriggerKind::Resource, 8, Some(&checkpoint)), 0);
    }

    #[test]
    fn test_parse_cert_info_invalid_pem() {
        let info = parse_cert_info("not a pem");
        assert!(info.serial.is_none());
        assert!(info.not_after.is_none());
    }

    #[test]
    fn test_parse_cert_info_valid_self_signed() {
        // Generate a self-signed cert for testing
        let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let params = rcgen::CertificateParams::new(vec!["test.example.com".to_string()]).unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let pem = cert.pem();

        let info = parse_cert_info(&pem);
        assert!(info.serial.is_some(), "Should have serial");
        assert!(info.not_after.is_some(), "Should have not_after");

        // Verify not_after is valid RFC 3339 (parseable by chrono)
        let not_after_str = info.not_after.unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(&not_after_str);
        assert!(
            parsed.is_ok(),
            "not_after should be valid RFC 3339, got: {}",
            not_after_str
        );
    }
}
