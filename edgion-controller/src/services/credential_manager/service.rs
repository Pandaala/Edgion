//! Restartable, leader-fenced credential acquisition service.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use edgion_common::link_sys_core::outbound_tls::PreparedLinkSysTls;
use edgion_common::time::format_rfc3339;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use k8s_openapi::api::core::v1::Secret;
use kube::api::{Api, ListParams};
use kube::Client;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::task::AbortHandle;
use uuid::Uuid;

use crate::conf_mgr::conf_center::kubernetes::LeaderFence;
use crate::conf_mgr::sync_runtime::resource_processor::credential_source_state::{
    CredentialManagerResult, DesiredCredentialConsumer, DesiredCredentialSource,
};
use crate::conf_mgr::{ControllerRuntime, LeaderHandle};

use super::memory::GeneratedSecretEntry;
use super::providers::oauth2::{BootstrapCredentials, OAuth2FailureKind, OAuth2Provider, OAuth2ProviderError};
use super::scheduler::{refresh_delay, retry_delay, usable_lifetime, EXPIRY_MARGIN, MIN_USABILITY};
use super::secret_io::{self, DeleteExpectation, ManagedSecretIdentity, Ownership, Publication, PublicationMetadata};
use edgion_resources::credentials::{BearerResolver, SecretMaterialResolver};
use edgion_resources::duration::parse_duration;
use edgion_resources::resources::link_sys::CredentialSourceState;
use zeroize::Zeroizing;

const WAKE_INTERVAL: Duration = Duration::from_secs(1);
const GLOBAL_CONCURRENCY: usize = 8;

static SERVICE: Mutex<Option<ServiceHandle>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

struct ServiceHandle {
    generation: u64,
    abort: AbortHandle,
}

/// Start the process-global leader-only credential manager.
///
/// The registry and Secret cache are already fully populated when the
/// Kubernetes center calls this function. The namespace list is accepted as a
/// lifecycle boundary for parity with the other Kubernetes services; desired
/// consumers already carry their effective namespaces.
pub fn start_credential_manager_service(
    client: Client,
    watch_namespaces: Vec<String>,
    runtime: Arc<ControllerRuntime>,
    leader: LeaderHandle,
) {
    spawn_worker(
        CredentialManagerBackend::Kubernetes(Arc::new(KubernetesCredentialIo {
            client,
            leader,
            watch_namespaces,
        })),
        runtime,
    );
}

/// Start the kube-less acquire loop used by FileSystem and Etcd centers.
///
/// Process-local generation is the only fence; the process is treated as
/// always-leader. Persist-on sources are not acquired (they stay
/// `Accepted=False` outside Kubernetes). Secret sweep/delete paths are skipped.
pub fn start_memory_credential_manager_service(runtime: Arc<ControllerRuntime>) {
    spawn_worker(CredentialManagerBackend::MemoryOnly, runtime);
}

fn spawn_worker(backend: CredentialManagerBackend, runtime: Arc<ControllerRuntime>) {
    let mut service = SERVICE.lock();
    if service.is_some() {
        tracing::warn!(
            component = "credential_manager",
            "Credential manager already running; stop it before restarting"
        );
        return;
    }
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let kube = matches!(backend, CredentialManagerBackend::Kubernetes(_));
    let task = tokio::spawn(run_worker(backend, runtime, generation));
    *service = Some(ServiceHandle {
        generation,
        abort: task.abort_handle(),
    });
    drop(service);
    crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("credential_manager").set(1.0);

    crate::services::supervision::observe_worker("credential_manager", task, move || {
        let current = SERVICE.lock().as_ref().map(|handle| handle.generation) == Some(generation);
        if current {
            crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("credential_manager").set(0.0);
        }
        current
    });
    tracing::info!(
        component = "credential_manager",
        generation,
        kube,
        "Credential manager started"
    );
}

/// Stop the current generation. Aborting the worker also cancels all issuer and
/// Kubernetes I/O futures owned by it.
pub fn stop_credential_manager_service() {
    if let Some(handle) = SERVICE.lock().take() {
        handle.abort.abort();
        crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("credential_manager").set(0.0);
        tracing::info!(
            component = "credential_manager",
            generation = handle.generation,
            "Credential manager stopped"
        );
    }
}

fn is_current_generation(generation: u64) -> bool {
    SERVICE.lock().as_ref().map(|handle| handle.generation) == Some(generation)
}

#[derive(Clone)]
struct KubernetesCredentialIo {
    client: Client,
    leader: LeaderHandle,
    watch_namespaces: Vec<String>,
}

#[derive(Clone)]
enum CredentialManagerBackend {
    Kubernetes(Arc<KubernetesCredentialIo>),
    MemoryOnly,
}

impl CredentialManagerBackend {
    fn is_leader(&self) -> bool {
        match self {
            Self::Kubernetes(io) => io.leader.is_leader(),
            Self::MemoryOnly => true,
        }
    }

    fn kube(&self) -> Option<(&Client, &LeaderHandle)> {
        match self {
            Self::Kubernetes(io) => Some((&io.client, &io.leader)),
            Self::MemoryOnly => None,
        }
    }

    /// Persist-on requires Secret I/O and is Kubernetes-only. Memory-only
    /// workers acquire persist-off sources and skip persist-on ones.
    fn may_acquire(&self, source: &DesiredCredentialSource) -> bool {
        match self {
            Self::Kubernetes(_) => true,
            Self::MemoryOnly => !source.persist,
        }
    }
}

async fn generation_and_fence_hold(
    generation: u64,
    leader: Option<&LeaderHandle>,
    fence: Option<&LeaderFence>,
) -> bool {
    if !is_current_generation(generation) {
        return false;
    }
    let fence_holds = match (leader, fence) {
        (Some(leader), Some(fence)) => leader.validate_fence(fence).await,
        (None, None) => true,
        _ => false,
    };
    fence_holds && is_current_generation(generation)
}

#[derive(Default)]
struct SourceState {
    identity: String,
    input_revision: String,
    consumer_revision: String,
    persist: bool,
    next_attempt: Option<Instant>,
    failure_count: u32,
    expires_at: Option<Instant>,
    expired: bool,
    publications: HashMap<String, DeleteExpectation>,
    retained: Option<RetainedPublication>,
    publication_retry: bool,
    conflicted_consumer_count: u32,
    last_publication_generation: Option<String>,
    last_issued_at: Option<SystemTime>,
    last_valid_until: Option<SystemTime>,
}

#[derive(Clone)]
pub(crate) struct RetainedPublication {
    access_token: String,
    publication_generation: String,
    issued_at: SystemTime,
    valid_until: SystemTime,
    expires_at: Instant,
    next_refresh_at: Instant,
}

enum JobResult {
    Reconciled {
        key: String,
        identity: String,
        input_revision: String,
        consumer_revision: String,
        consumer_binding_revision: String,
        result: Box<Result<ReconcileSuccess, ReconcileFailure>>,
    },
    Cleaned {
        key: String,
        remaining: HashMap<String, DeleteExpectation>,
    },
}

struct ReconcileSuccess {
    next_delay: Duration,
    expires_at: Option<Instant>,
    publications: HashMap<String, DeleteExpectation>,
    retained: RetainedPublication,
}

#[derive(Clone, Copy)]
enum FailureSchedule {
    Transient,
    Rotation,
}

struct ReconcileFailure {
    retry_after: Option<Duration>,
    schedule: FailureSchedule,
    publications: Option<HashMap<String, DeleteExpectation>>,
    retained: Option<Box<RetainedPublication>>,
    consumer_conflict: bool,
}

impl ReconcileFailure {
    fn transient(retry_after: Option<Duration>) -> Self {
        Self {
            retry_after,
            schedule: FailureSchedule::Transient,
            publications: None,
            retained: None,
            consumer_conflict: false,
        }
    }

    fn rotation() -> Self {
        Self {
            retry_after: None,
            schedule: FailureSchedule::Rotation,
            publications: None,
            retained: None,
            consumer_conflict: false,
        }
    }

    fn with_partial_publication(
        mut self,
        publications: HashMap<String, DeleteExpectation>,
        retained: RetainedPublication,
    ) -> Self {
        self.publications = Some(publications);
        self.retained = Some(Box::new(retained));
        self
    }
}

async fn run_worker(backend: CredentialManagerBackend, runtime: Arc<ControllerRuntime>, generation: u64) {
    let mut sweep_complete = match &backend {
        CredentialManagerBackend::Kubernetes(io) => {
            startup_sweep(&io.client, &io.watch_namespaces, &runtime, &io.leader, generation).await
        }
        CredentialManagerBackend::MemoryOnly => true,
    };
    let mut interval = tokio::time::interval(WAKE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut states = HashMap::<String, SourceState>::new();
    let mut in_flight = HashSet::<String>::new();
    let mut jobs = FuturesUnordered::<BoxFuture<'static, JobResult>>::new();

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if !is_current_generation(generation) || !backend.is_leader() {
                    continue;
                }
                if !sweep_complete {
                    sweep_complete = match &backend {
                        CredentialManagerBackend::Kubernetes(io) => {
                            startup_sweep(&io.client, &io.watch_namespaces, &runtime, &io.leader, generation).await
                        }
                        CredentialManagerBackend::MemoryOnly => true,
                    };
                }
                let sources = runtime.credential_source_registry.sources();
                let source_keys: HashSet<_> = sources.iter().map(DesiredCredentialSource::logical_key).collect();

                for removed in states.keys().filter(|key| !source_keys.contains(*key)).cloned().collect::<Vec<_>>() {
                    let old = states.get_mut(&removed).expect("key came from states");
                    // Credential bytes do not share the lifetime of fenced
                    // leftover Secret cleanup. Source teardown clears them
                    // even while a cleanup job is in flight or capacity-bound.
                    forget_unregistered_source_memory(&runtime, old);
                    if in_flight.contains(&removed) {
                        continue;
                    }
                    if old.publications.is_empty() {
                        states.remove(&removed);
                        continue;
                    }
                    if jobs.len() >= GLOBAL_CONCURRENCY {
                        continue;
                    }
                    match kube_io(&backend) {
                        Some((client, leader)) => {
                            let publications = std::mem::take(&mut old.publications);
                            in_flight.insert(removed.clone());
                            jobs.push(Box::pin(cleanup_removed_source(
                                removed, publications, client, leader, generation,
                            )));
                        }
                        None => {
                            states.remove(&removed);
                        }
                    }
                }

                for source in sources {
                    let key = source.logical_key();
                    let identity = source.identity_key();
                    if let Some(state) = states.get(&key) {
                        forget_replaced_identity_memory(&runtime, &state.identity, &identity);
                    }
                    if jobs.len() >= GLOBAL_CONCURRENCY {
                        continue;
                    }
                    if in_flight.contains(&key) {
                        continue;
                    }
                    let consumers = runtime.credential_source_registry.consumers_for_source(&key);
                    let input_revision = source_input_revision(&source);
                    let consumer_revision = consumer_revision(
                        &consumers,
                        runtime
                            .credential_source_registry
                            .reconcile_epoch_for_source(&key),
                    );
                    if !states.contains_key(&key) {
                        states.insert(
                            key.clone(),
                            recover_source_state(&runtime, &source, &consumers, &input_revision),
                        );
                    }
                    let state = states.entry(key.clone()).or_default();
                    let desired_changed = state.identity != identity
                        || state.input_revision != input_revision
                        || state.consumer_revision != consumer_revision
                        || state.persist != source.persist;
                    if state.identity != identity || state.input_revision != input_revision {
                        state.expired = false;
                    }
                    let expired = state.expires_at.is_some_and(|deadline| deadline <= Instant::now());
                    if expired {
                        let notify_expiry = clear_expired_credential(&runtime, &source, state);
                        if !state.publications.is_empty() && jobs.len() < GLOBAL_CONCURRENCY {
                            match kube_io(&backend) {
                                Some((client, leader)) => {
                                    let publications = std::mem::take(&mut state.publications);
                                    in_flight.insert(key.clone());
                                    jobs.push(Box::pin(cleanup_removed_source(
                                        key, publications, client, leader, generation,
                                    )));
                                }
                                None => {
                                    state.publications.clear();
                                }
                            }
                        } else if !state.publications.is_empty() {
                            // Keep the cleanup eligible when all job slots are busy.
                            state.expires_at = Some(Instant::now());
                        }
                        publish_manager_result(&runtime, &source, state).await;
                        if notify_expiry {
                            notify_consumers(
                                &runtime,
                                &consumers,
                                format!(
                                    "expired:{}",
                                    state.last_publication_generation.as_deref().unwrap_or_default()
                                ),
                            )
                            .await;
                        }
                        continue;
                    }
                    let due = state.next_attempt.is_none_or(|deadline| deadline <= Instant::now());
                    if !(desired_changed || due) {
                        continue;
                    }
                    if consumers.is_empty() {
                        if !state.publications.is_empty() {
                            match kube_io(&backend) {
                                Some((client, leader)) => {
                                    let publications = std::mem::take(&mut state.publications);
                                    in_flight.insert(key.clone());
                                    jobs.push(Box::pin(cleanup_removed_source(
                                        key.clone(), publications, client, leader, generation,
                                    )));
                                }
                                None => {
                                    state.publications.clear();
                                }
                            }
                        }
                        clear_memory_identity(&runtime, &state.identity);
                        clear_source_memory(&runtime, &source);
                        state.next_attempt = None;
                        state.expires_at = None;
                        state.retained = None;
                        state.publication_retry = false;
                        continue;
                    }
                    if !backend.may_acquire(&source) {
                        continue;
                    }
                    let previous_publications = state.publications.clone();
                    let retained = retained_for_attempt(
                        state.persist,
                        source.persist,
                        state.identity == identity,
                        state.input_revision == input_revision,
                        state.publication_retry || !due,
                        state.retained.clone(),
                    );
                    in_flight.insert(key.clone());
                    jobs.push(Box::pin(reconcile_source(
                        source,
                        consumers,
                        previous_publications,
                        backend.clone(),
                        runtime.clone(),
                        generation,
                        input_revision,
                        consumer_revision,
                        retained,
                    )));
                }
            }
            Some(result) = jobs.next() => {
                match result {
                    JobResult::Cleaned { key, remaining } => {
                        in_flight.remove(&key);
                        if !remaining.is_empty() {
                            if let Some(state) = states.get_mut(&key) {
                                state.publications.extend(remaining);
                                state.expires_at = Some(Instant::now());
                            }
                        } else if runtime.credential_source_registry.source(&key).is_none() {
                            if let Some(old) = states.remove(&key) {
                                forget_unregistered_source_memory(&runtime, &old);
                            }
                        }
                        if let (Some(source), Some(state)) = (
                            runtime.credential_source_registry.source(&key),
                            states.get(&key),
                        ) {
                            publish_manager_result(&runtime, &source, state).await;
                        }
                    }
                    JobResult::Reconciled { key, identity, input_revision, consumer_revision, consumer_binding_revision, result } => {
                        in_flight.remove(&key);
                        if !is_current_generation(generation)
                            || !reconciled_consumers_current(&runtime, &key, &consumer_binding_revision) {
                            discard_stale_reconciled(&runtime, &identity);
                            preserve_reconciled_cleanup(&mut states, key, *result);
                            continue;
                        }
                        let Some(current) = take_current_for_reconciled(&runtime, &key, &identity, &input_revision) else {
                            preserve_reconciled_cleanup(&mut states, key, *result);
                            continue;
                        };
                        let state = states.entry(key).or_default();
                        state.identity = identity;
                        state.input_revision = input_revision;
                        state.consumer_revision = consumer_revision;
                        match *result {
                            Ok(success) => {
                                state.failure_count = 0;
                                state.persist = current.persist;
                                state.expired = false;
                                state.next_attempt = Some(Instant::now() + success.next_delay);
                                state.expires_at = success.expires_at;
                                state.publications = success.publications;
                                state.retained = Some(success.retained);
                                if let Some(retained) = state.retained.as_ref() {
                                    state.last_publication_generation =
                                        Some(retained.publication_generation.clone());
                                    state.last_issued_at = Some(retained.issued_at);
                                    state.last_valid_until = Some(retained.valid_until);
                                }
                                state.publication_retry = false;
                                state.conflicted_consumer_count = 0;
                            }
                            Err(mut failure) => {
                                state.conflicted_consumer_count = u32::from(failure.consumer_conflict);
                                if let Some(publications) = failure.publications.take() {
                                    state.publications = publications;
                                }
                                if let Some(retained) = failure.retained.take() {
                                    state.expired = false;
                                    state.expires_at = Some(retained.expires_at);
                                    state.retained = Some(*retained);
                                    state.publication_retry = true;
                                }
                                state.failure_count = state.failure_count.saturating_add(1);
                                let rotation = &current.config.rotation;
                                let initial = parse_duration(&rotation.retry_initial_backoff).unwrap_or(Duration::from_secs(2));
                                let maximum = parse_duration(&rotation.retry_max_backoff).unwrap_or(Duration::from_secs(120));
                                let delay = match failure.schedule {
                                    FailureSchedule::Transient => retry_delay(
                                        &current.identity_key(),
                                        state.failure_count,
                                        initial,
                                        maximum,
                                        failure.retry_after,
                                    ),
                                    FailureSchedule::Rotation => parse_duration(&rotation.interval)
                                        .unwrap_or(Duration::from_secs(900)),
                                };
                                state.next_attempt = Some(Instant::now() + delay);
                            }
                        }
                        publish_manager_result(&runtime, &current, state).await;
                    }
                }
            }
            else => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn reconcile_source(
    source: DesiredCredentialSource,
    consumers: Vec<DesiredCredentialConsumer>,
    previous_publications: HashMap<String, DeleteExpectation>,
    backend: CredentialManagerBackend,
    runtime: Arc<ControllerRuntime>,
    generation: u64,
    input_revision: String,
    consumer_revision: String,
    retained: Option<RetainedPublication>,
) -> JobResult {
    let key = source.logical_key();
    let identity = source.identity_key();
    // Notifications emitted by this job advance the retry epoch. Only actual
    // desired bindings belong in the completion acceptance fence.
    let consumer_binding_revision = self::consumer_revision(&consumers, 0);
    let result = acquire_and_publish(
        &source,
        &consumers,
        previous_publications,
        &backend,
        &runtime,
        generation,
        &input_revision,
        retained,
    )
    .await;
    JobResult::Reconciled {
        key,
        identity,
        input_revision,
        consumer_revision,
        consumer_binding_revision,
        result: Box::new(result),
    }
}

#[allow(clippy::too_many_arguments)]
async fn acquire_and_publish(
    source: &DesiredCredentialSource,
    consumers: &[DesiredCredentialConsumer],
    mut previous_publications: HashMap<String, DeleteExpectation>,
    backend: &CredentialManagerBackend,
    runtime: &ControllerRuntime,
    generation: u64,
    input_revision: &str,
    retained: Option<RetainedPublication>,
) -> Result<ReconcileSuccess, ReconcileFailure> {
    if matches!(backend, CredentialManagerBackend::MemoryOnly) && source.persist {
        return Err(ReconcileFailure::rotation());
    }

    let (kube_client, kube_leader) = match backend.kube() {
        Some((client, leader)) => (Some(client), Some(leader)),
        None => (None, None),
    };
    let fence = match kube_leader {
        Some(leader) => Some(
            leader
                .current_fence()
                .ok_or_else(|| ReconcileFailure::transient(None))?,
        ),
        None => None,
    };
    if !generation_and_fence_hold(generation, kube_leader, fence.as_ref()).await
        || !source_is_current(runtime, source)
        || !consumers
            .iter()
            .all(|consumer| consumer.matches_source(source) && consumer_is_current(runtime, consumer))
    {
        return Err(ReconcileFailure::transient(None));
    }

    let rotation = parse_duration(&source.config.rotation.interval).map_err(|_| ReconcileFailure::rotation())?;
    let (retained, next_delay) = if let Some(retained) = retained.filter(|value| value.expires_at > Instant::now()) {
        let next_delay = retained.next_refresh_at.saturating_duration_since(Instant::now());
        (retained, next_delay)
    } else {
        let active = bootstrap_for_role(runtime, source, "active-bootstrap")?;
        let previous = bootstrap_for_role(runtime, source, "previous-bootstrap").ok();
        let request_timeout =
            parse_duration(&source.config.rotation.request_timeout).map_err(|_| ReconcileFailure::rotation())?;
        let retry_cap =
            parse_duration(&source.config.rotation.retry_max_backoff).map_err(|_| ReconcileFailure::rotation())?;
        let prepared =
            PreparedLinkSysTls::from_link_sys(source.config.owner()).map_err(|_| ReconcileFailure::rotation())?;
        prepared
            .validate_active_inheritance()
            .map_err(|_| ReconcileFailure::rotation())?;
        let resolver = match kube_client {
            Some(client) => crate::services::dns_resolver::service::controller_default_resolver(client).await,
            None => crate::services::dns_resolver::service::controller_default_resolver_without_kube(),
        };
        let provider = OAuth2Provider::new(&prepared, request_timeout, retry_cap, resolver)
            .map_err(|_| ReconcileFailure::rotation())?;

        // The exact Lease tuple is re-read immediately before issuer I/O.
        if !generation_and_fence_hold(generation, kube_leader, fence.as_ref()).await
            || !source_is_current(runtime, source)
        {
            return Err(ReconcileFailure::transient(None));
        }
        let active_started_wall = SystemTime::now();
        let active_started_mono = Instant::now();
        let (token, request_started_wall, request_started_mono) = match provider.acquire(&active).await {
            Ok(token) => (token, active_started_wall, active_started_mono),
            Err(error) if error.kind == OAuth2FailureKind::InvalidClient && previous.is_some() => {
                if !generation_and_fence_hold(generation, kube_leader, fence.as_ref()).await
                    || !source_is_current(runtime, source)
                {
                    return Err(ReconcileFailure::transient(None));
                }
                let previous_started_wall = SystemTime::now();
                let previous_started_mono = Instant::now();
                let token = provider
                    .acquire(previous.as_ref().unwrap())
                    .await
                    .map_err(provider_failure)?;
                (token, previous_started_wall, previous_started_mono)
            }
            Err(error) => return Err(provider_failure(error)),
        };
        let Some(usable) = usable_lifetime(token.expires_in) else {
            return Err(ReconcileFailure::rotation());
        };
        let issued_at = SystemTime::now();
        let valid_until = request_started_wall
            .checked_add(usable)
            .ok_or_else(ReconcileFailure::rotation)?;
        let expires_at = request_started_mono
            .checked_add(usable)
            .ok_or_else(ReconcileFailure::rotation)?;
        if valid_until
            .duration_since(issued_at)
            .ok()
            .is_none_or(|remaining| remaining < MIN_USABILITY)
        {
            return Err(ReconcileFailure::transient(None));
        }
        let publication_generation = Uuid::new_v4().to_string();
        let refresh_at = request_started_mono
            .checked_add(refresh_delay(
                &format!("{}/{}", source.identity_key(), publication_generation),
                token.expires_in,
                rotation,
            ))
            .ok_or_else(ReconcileFailure::rotation)?;
        (
            RetainedPublication {
                access_token: token.access_token,
                publication_generation,
                issued_at,
                valid_until,
                expires_at,
                next_refresh_at: refresh_at,
            },
            refresh_at.saturating_duration_since(Instant::now()),
        )
    };
    let mut publications = HashMap::new();

    if !source.persist {
        if let (Some(leader), Some(fence)) = (kube_leader, fence.as_ref()) {
            if !leader.validate_fence(fence).await {
                return Err(ReconcileFailure::transient(None));
            }
        }
        if !consumers
            .iter()
            .all(|consumer| consumer.matches_source(source) && consumer_is_current(runtime, consumer))
        {
            return Err(ReconcileFailure::transient(None));
        }
        try_publish_persist_off(runtime, source, &retained, generation)?;
        notify_consumers(runtime, consumers, retained.publication_generation.clone()).await;
    } else {
        let client = kube_client.ok_or_else(ReconcileFailure::rotation)?;
        let leader = kube_leader.ok_or_else(ReconcileFailure::rotation)?;
        let fence = fence.as_ref().ok_or_else(ReconcileFailure::rotation)?;
        for consumer in consumers {
            if !consumer_is_current(runtime, consumer)
                || !consumer.matches_source(source)
                || !source_is_current(runtime, source)
                || !is_current_generation(generation)
            {
                publications.extend(previous_publications);
                return Err(ReconcileFailure::transient(None).with_partial_publication(publications, retained));
            }
            let expectation = match publish_consumer(
                client,
                consumer,
                source,
                &retained.access_token,
                input_revision,
                &retained.publication_generation,
                retained.issued_at,
                retained.valid_until,
                fence,
                runtime,
                leader,
                generation,
            )
            .await
            {
                Ok(expectation) => expectation,
                Err(failure) => {
                    publications.extend(previous_publications);
                    return Err(failure.with_partial_publication(publications, retained));
                }
            };
            previous_publications.remove(&consumer.logical_key());
            publications.insert(consumer.logical_key(), expectation);
        }
    }

    // Bindings removed or switched while the old publication was live are
    // deleted only with the old source/binding/UID expectations. Persist-off
    // still runs this loop so a true→false leftover Secret is fenced-deleted;
    // an always-persist-off source has an empty map and never constructs Api::<Secret>.
    // Kube-less workers skip Secret I/O entirely (no leftover Secrets exist).
    if let (Some(client), Some(leader), Some(fence)) = (kube_client, kube_leader, fence.as_ref()) {
        for (consumer_key, expectation) in previous_publications {
            if secret_io::delete_if_matches(client, &expectation, || async {
                if !generation_and_fence_hold(generation, Some(leader), Some(fence)).await
                    || !source_is_current(runtime, source)
                {
                    return Err(secret_io::SecretIoError::StaleOperation);
                }
                Ok(!publication_is_desired(runtime, &expectation))
            })
            .await
            .is_err()
            {
                publications.insert(consumer_key, expectation);
            }
        }
    }

    Ok(reconcile_ok(next_delay, retained, publications))
}

fn kube_io(backend: &CredentialManagerBackend) -> Option<(Client, LeaderHandle)> {
    match backend {
        CredentialManagerBackend::Kubernetes(io) => Some((io.client.clone(), io.leader.clone())),
        CredentialManagerBackend::MemoryOnly => None,
    }
}

/// Persist-off memory write. Re-checks worker generation immediately before
/// the insert so a stopped generation cannot publish after `runtime.reset()`.
#[allow(clippy::result_large_err)]
fn try_publish_persist_off(
    runtime: &ControllerRuntime,
    source: &DesiredCredentialSource,
    retained: &RetainedPublication,
    generation: u64,
) -> Result<(), ReconcileFailure> {
    if !is_current_generation(generation) || !source_is_current(runtime, source) {
        return Err(ReconcileFailure::transient(None));
    }
    if !memory_publish_allowed(runtime, source) {
        return Err(ReconcileFailure::transient(None));
    }
    publish_to_memory(runtime, source, retained);
    Ok(())
}

/// Leftover-delete failures stay in this map even when persist-off. Persist-off
/// `published_consumer_count` already ignores the map (`retained.is_some()`).
fn reconcile_ok(
    next_delay: Duration,
    retained: RetainedPublication,
    publications: HashMap<String, DeleteExpectation>,
) -> ReconcileSuccess {
    ReconcileSuccess {
        next_delay,
        expires_at: Some(retained.expires_at),
        publications,
        retained,
    }
}

fn discard_stale_reconciled(runtime: &ControllerRuntime, identity: &str) {
    clear_memory_identity(runtime, identity);
}

/// A stale job cannot publish retained material or status. Mutations that
/// already completed still supply exact old output authority for cleanup.
fn preserve_reconciled_cleanup(
    states: &mut HashMap<String, SourceState>,
    key: String,
    result: Result<ReconcileSuccess, ReconcileFailure>,
) {
    let publications = match result {
        Ok(success) => success.publications,
        Err(failure) => failure.publications.unwrap_or_default(),
    };
    if !publications.is_empty() {
        states.entry(key).or_default().publications.extend(publications);
    }
}

fn take_current_for_reconciled(
    runtime: &ControllerRuntime,
    key: &str,
    identity: &str,
    input_revision: &str,
) -> Option<DesiredCredentialSource> {
    let Some(current) = runtime.credential_source_registry.source(key) else {
        discard_stale_reconciled(runtime, identity);
        return None;
    };
    if current.identity_key() != identity
        || source_input_revision(&current) != input_revision
        || !source_is_current(runtime, &current)
    {
        discard_stale_reconciled(runtime, identity);
        return None;
    }
    Some(current)
}

fn memory_publish_allowed(runtime: &ControllerRuntime, source: &DesiredCredentialSource) -> bool {
    if source.persist || !runtime.single_publisher() {
        return false;
    }
    runtime
        .credential_source_registry
        .source(&source.logical_key())
        .is_none_or(|current| !current.persist)
}

pub(crate) fn publish_to_memory(
    runtime: &ControllerRuntime,
    source: &DesiredCredentialSource,
    retained: &RetainedPublication,
) {
    if !memory_publish_allowed(runtime, source) {
        return;
    }
    runtime.credential_memory.insert_oauth_publication(
        &source.identity_key(),
        GeneratedSecretEntry {
            key: retained.publication_generation.clone(),
            value: Zeroizing::new(retained.access_token.as_bytes().to_vec()),
            issued_at: format_rfc3339(retained.issued_at),
            valid_until: format_rfc3339(retained.valid_until),
            valid_until_mono: retained.expires_at,
        },
    );
}

fn clear_source_memory(runtime: &ControllerRuntime, source: &DesiredCredentialSource) {
    clear_memory_identity(runtime, &source.identity_key());
}

fn clear_expired_credential(
    runtime: &ControllerRuntime,
    source: &DesiredCredentialSource,
    state: &mut SourceState,
) -> bool {
    let recorded_identity = state.identity.clone();
    let current_identity = source.identity_key();
    let had_active_memory = runtime.credential_memory.active(&recorded_identity).is_some()
        || (recorded_identity != current_identity && runtime.credential_memory.active(&current_identity).is_some());
    clear_memory_identity(runtime, &recorded_identity);
    clear_memory_identity(runtime, &current_identity);
    let had_retained = state.retained.take().is_some();
    state.expires_at = None;
    state.expired = true;
    state.publication_retry = false;
    had_retained || had_active_memory
}

async fn notify_consumers(
    runtime: &ControllerRuntime,
    consumers: &[DesiredCredentialConsumer],
    observed_revision: String,
) {
    for consumer in consumers {
        if !runtime
            .credential_source_registry
            .request_consumer_reconcile_if_current(consumer, observed_revision.clone())
        {
            continue;
        }
        if let Some(processor) = runtime.processor_registry.get("EdgionPlugins") {
            processor.requeue(consumer.logical_key()).await;
        }
    }
}

fn clear_memory_identity(runtime: &ControllerRuntime, identity: &str) {
    if identity.is_empty() {
        return;
    }
    runtime.credential_memory.clear_source(identity);
}

fn forget_unregistered_source_memory(runtime: &ControllerRuntime, state: &SourceState) {
    clear_memory_identity(runtime, &state.identity);
}

fn forget_replaced_identity_memory(runtime: &ControllerRuntime, previous_identity: &str, next_identity: &str) {
    if previous_identity != next_identity {
        clear_memory_identity(runtime, previous_identity);
    }
}

fn maybe_clear_memory_after_fan_out(
    runtime: &ControllerRuntime,
    source: &DesiredCredentialSource,
    published_consumer_count: u32,
    desired_consumer_count: u32,
) {
    if source.persist && published_consumer_count == desired_consumer_count {
        clear_source_memory(runtime, source);
    }
}

fn retained_for_attempt(
    previous_persist: bool,
    next_persist: bool,
    identity_unchanged: bool,
    input_revision_unchanged: bool,
    publication_retry_or_not_due: bool,
    retained: Option<RetainedPublication>,
) -> Option<RetainedPublication> {
    if previous_persist != next_persist && identity_unchanged && input_revision_unchanged {
        return retained;
    }
    (publication_retry_or_not_due && identity_unchanged && input_revision_unchanged)
        .then_some(retained)
        .flatten()
}

fn recover_source_state(
    runtime: &ControllerRuntime,
    source: &DesiredCredentialSource,
    consumers: &[DesiredCredentialConsumer],
    input_revision: &str,
) -> SourceState {
    // Cleanup authority comes from exact old output metadata, independently of
    // whether the current owner can still adopt its credential material.
    let mut publications = HashMap::new();
    for consumer in consumers {
        let Ok(target) = consumer.output_target() else {
            continue;
        };
        let Some(target_namespace) = target.scope().namespace() else {
            continue;
        };
        let Some(secret) = runtime.secret_store.get(Some(target_namespace), target.name()) else {
            continue;
        };
        let Some(expected) = secret_io::delete_expectation_from_secret(&secret) else {
            continue;
        };
        if expected.identity.consumer().namespace == target_namespace
            && expected.identity.consumer().name == target.name()
            && expected.identity.consumer().uid == consumer.uid
            && expected.identity.source().name == source.logical_key()
            && expected.identity.source().uid == source.uid
        {
            publications.insert(consumer.logical_key(), expected);
        }
    }
    if !source.persist
        || !source_is_current(runtime, source)
        || source_input_revision(source) != input_revision
        || !consumers
            .iter()
            .all(|consumer| consumer_is_current(runtime, consumer) && consumer.matches_source(source))
    {
        return SourceState {
            publications,
            ..Default::default()
        };
    }
    let mut publication_ids = HashSet::new();
    let mut recovered: Option<RetainedPublication> = None;

    for consumer in consumers {
        let Ok(target) = consumer.output_target() else {
            continue;
        };
        let Some(target_namespace) = target.scope().namespace() else {
            continue;
        };
        let Some(secret) = runtime.secret_store.get(Some(target_namespace), target.name()) else {
            continue;
        };
        if consumer.reference.validate_managed_credential_secret(&secret).is_err() {
            continue;
        }
        let Ok(identity) = ManagedSecretIdentity::from_current_pair(consumer, source) else {
            continue;
        };
        if secret_io::classify_ownership(&secret, &identity) != Ownership::Owned {
            continue;
        }
        let Some(annotations) = secret.metadata.annotations.as_ref() else {
            continue;
        };
        if annotations
            .get(secret_io::ANNOTATION_SOURCE_INPUT_REVISION)
            .map(String::as_str)
            != Some(input_revision)
        {
            continue;
        }
        let Some(publication_generation) = annotations.get(secret_io::ANNOTATION_PUBLICATION_GENERATION).cloned()
        else {
            continue;
        };
        publication_ids.insert(publication_generation.clone());

        let Some(data) = secret.data.as_ref() else {
            return SourceState {
                publications,
                ..Default::default()
            };
        };
        let Ok(material) = BearerResolver.parse(data) else {
            return SourceState {
                publications,
                ..Default::default()
            };
        };
        let Some(valid_until) = annotations
            .get(secret_io::ANNOTATION_VALID_UNTIL)
            .and_then(|value| parse_rfc3339_system_time(value))
        else {
            return SourceState {
                publications,
                ..Default::default()
            };
        };
        let now_wall = SystemTime::now();
        let Some(remaining) = valid_until.duration_since(now_wall).ok() else {
            return SourceState {
                publications,
                ..Default::default()
            };
        };
        if remaining < MIN_USABILITY {
            return SourceState {
                publications,
                ..Default::default()
            };
        }
        let Some(expires_at) = Instant::now().checked_add(remaining) else {
            return SourceState {
                publications,
                ..Default::default()
            };
        };
        let Some(issued_at) = annotations
            .get(secret_io::ANNOTATION_ISSUED_AT)
            .and_then(|value| parse_rfc3339_system_time(value))
        else {
            return SourceState {
                publications,
                ..Default::default()
            };
        };
        let candidate = RetainedPublication {
            access_token: material.token,
            publication_generation,
            issued_at,
            valid_until,
            expires_at,
            // Recovered below after the source rotation policy is available.
            next_refresh_at: Instant::now(),
        };
        if recovered.as_ref().is_some_and(|current| {
            current.publication_generation != candidate.publication_generation
                || current.access_token != candidate.access_token
                || current.valid_until != candidate.valid_until
        }) {
            return SourceState {
                publications,
                ..Default::default()
            };
        }
        recovered = Some(candidate);
        if let Some(binding_revision) = annotations.get(secret_io::ANNOTATION_BINDING_REVISION).cloned() {
            let Ok(expectation) = secret_io::capture_delete_expectation(&secret, identity, binding_revision) else {
                return SourceState {
                    publications,
                    ..Default::default()
                };
            };
            publications.insert(consumer.logical_key(), expectation);
        }
    }

    if publication_ids.len() != 1 {
        return SourceState {
            publications,
            ..Default::default()
        };
    }
    let Some(retained) = recovered else {
        return SourceState {
            publications,
            ..Default::default()
        };
    };
    let rotation = parse_duration(&source.config.rotation.interval).unwrap_or(Duration::from_secs(900));
    let jitter_key = format!("{}/{}", source.identity_key(), retained.publication_generation);
    let next_refresh_at = recovered_refresh_at(
        retained.issued_at,
        retained.valid_until,
        rotation,
        &jitter_key,
        SystemTime::now(),
        Instant::now(),
    );
    let mut retained = retained;
    retained.next_refresh_at = next_refresh_at;
    let last_publication_generation = Some(retained.publication_generation.clone());
    let last_issued_at = Some(retained.issued_at);
    let last_valid_until = Some(retained.valid_until);
    SourceState {
        identity: source.identity_key(),
        input_revision: input_revision.to_string(),
        // Force one fenced republish so every replica is claimed by the new
        // leader term and stale/missing binding metadata converges.
        consumer_revision: String::new(),
        next_attempt: Some(next_refresh_at),
        failure_count: 0,
        expires_at: Some(retained.expires_at),
        expired: false,
        publications,
        retained: Some(retained),
        publication_retry: false,
        conflicted_consumer_count: 0,
        last_publication_generation,
        last_issued_at,
        last_valid_until,
        persist: true,
    }
}

fn parse_rfc3339_system_time(value: &str) -> Option<SystemTime> {
    let timestamp = DateTime::parse_from_rfc3339(value).ok()?.with_timezone(&Utc);
    Some(SystemTime::from(timestamp))
}

fn recovered_refresh_at(
    issued_at: SystemTime,
    valid_until: SystemTime,
    rotation: Duration,
    jitter_key: &str,
    now_wall: SystemTime,
    now_mono: Instant,
) -> Instant {
    let usable_lifetime = valid_until.duration_since(issued_at).unwrap_or(Duration::ZERO);
    let provider_lifetime = usable_lifetime.saturating_add(EXPIRY_MARGIN);
    let refresh_wall = issued_at
        .checked_add(refresh_delay(jitter_key, provider_lifetime, rotation))
        .unwrap_or(issued_at);
    let until_refresh = refresh_wall.duration_since(now_wall).unwrap_or(Duration::ZERO);
    now_mono + until_refresh
}

async fn startup_sweep(
    client: &Client,
    watch_namespaces: &[String],
    runtime: &ControllerRuntime,
    leader: &LeaderHandle,
    generation: u64,
) -> bool {
    let params = ListParams::default().labels(&format!(
        "{}={}",
        secret_io::LABEL_MANAGED_BY,
        secret_io::MANAGED_BY_VALUE
    ));
    let listed = if watch_namespaces.is_empty() {
        Api::<Secret>::all(client.clone())
            .list(&params)
            .await
            .map(|list| list.items)
    } else {
        let mut items = Vec::new();
        let mut failure = None;
        for namespace in watch_namespaces {
            match Api::<Secret>::namespaced(client.clone(), namespace).list(&params).await {
                Ok(list) => items.extend(list.items),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        failure.map_or(Ok(items), Err)
    };
    let Ok(secrets) = listed else {
        tracing::warn!(
            component = "credential_manager",
            "Managed Secret startup sweep could not list targets"
        );
        return false;
    };
    let Some(fence) = leader.current_fence() else {
        return false;
    };
    for secret in secrets {
        let Some(expectation) = secret_io::delete_expectation_from_secret(&secret) else {
            continue;
        };
        let still_desired = publication_is_desired(runtime, &expectation);
        if still_desired {
            continue;
        }
        if secret_io::delete_if_matches(client, &expectation, || async {
            if !generation_and_fence_hold(generation, Some(leader), Some(&fence)).await {
                return Err(secret_io::SecretIoError::StaleOperation);
            }
            Ok(!publication_is_desired(runtime, &expectation))
        })
        .await
        .is_err()
        {
            tracing::warn!(
                component = "credential_manager",
                "Managed Secret startup cleanup failed"
            );
            return false;
        }
    }
    true
}

fn publication_is_desired(runtime: &ControllerRuntime, expected: &DeleteExpectation) -> bool {
    let key = format!(
        "{}/{}",
        expected.identity.consumer().namespace,
        expected.identity.consumer().name
    );
    let Some(consumer) = runtime.credential_source_registry.consumer(&key) else {
        return false;
    };
    let Some(source) = runtime.credential_source_registry.source(&consumer.source_key) else {
        return false;
    };
    source.persist
        && source_is_current(runtime, &source)
        && consumer_is_current(runtime, &consumer)
        && consumer.matches_source(&source)
        && consumer.uid == expected.identity.consumer().uid
        && consumer.generation == expected.identity.consumer().generation
        && consumer.source_key == expected.identity.source().name
        && consumer.source_uid == expected.identity.source().uid
        && consumer.source_generation == expected.identity.source().generation
        && consumer.binding_revision() == expected.binding_revision
        && source_input_revision(&source) == expected.source_input_revision
}

fn bootstrap_for_role(
    runtime: &ControllerRuntime,
    source: &DesiredCredentialSource,
    role: &str,
) -> Result<BootstrapCredentials, ReconcileFailure> {
    let input = source
        .secret_inputs
        .iter()
        .find(|input| input.role == role)
        .ok_or_else(ReconcileFailure::rotation)?;
    let secret = runtime
        .secret_store
        .get(Some(&input.namespace), &input.name)
        .ok_or_else(|| ReconcileFailure::transient(None))?;
    if secret.metadata.uid != input.uid || secret.metadata.resource_version != input.resource_version {
        return Err(ReconcileFailure::transient(None));
    }
    BootstrapCredentials::parse(secret.data.as_ref().unwrap_or(&BTreeMap::new()))
        .map_err(|_| ReconcileFailure::rotation())
}

fn provider_failure(error: OAuth2ProviderError) -> ReconcileFailure {
    match error.kind {
        OAuth2FailureKind::Transient => ReconcileFailure::transient(error.retry_after),
        OAuth2FailureKind::InvalidClient | OAuth2FailureKind::Permanent => ReconcileFailure::rotation(),
    }
}

fn secret_io_failure(error: secret_io::SecretIoError) -> ReconcileFailure {
    match error {
        secret_io::SecretIoError::Kubernetes(_) | secret_io::SecretIoError::StaleOperation => {
            ReconcileFailure::transient(None)
        }
        secret_io::SecretIoError::TargetSecretConflict => {
            let mut failure = ReconcileFailure::rotation();
            failure.consumer_conflict = true;
            failure
        }
        _ => ReconcileFailure::rotation(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn publish_consumer(
    client: &Client,
    consumer: &DesiredCredentialConsumer,
    source: &DesiredCredentialSource,
    access_token: &str,
    input_revision: &str,
    publication_generation: &str,
    issued_at: SystemTime,
    valid_until: SystemTime,
    fence: &LeaderFence,
    runtime: &ControllerRuntime,
    leader: &LeaderHandle,
    generation: u64,
) -> Result<DeleteExpectation, ReconcileFailure> {
    if !consumer.matches_source(source) {
        return Err(ReconcileFailure::transient(None));
    }
    let identity = ManagedSecretIdentity::from_current_pair(consumer, source).map_err(secret_io_failure)?;
    let binding_revision = consumer.binding_revision();
    let metadata = PublicationMetadata {
        publication_generation: publication_generation.to_string(),
        source_input_revision: input_revision.to_string(),
        binding_revision: binding_revision.clone(),
        issued_at: format_rfc3339(issued_at),
        valid_until: format_rfc3339(valid_until),
        leader_term: fence.term.clone(),
    };
    let publication = Publication {
        identity: &identity,
        metadata: &metadata,
        bearer_token: access_token.as_bytes(),
    };
    let current = secret_io::get_optional(client, &identity)
        .await
        .map_err(secret_io_failure)?;
    let still_current = || {
        is_current_generation(generation)
            && source_is_current(runtime, source)
            && consumer_is_current(runtime, consumer)
            && consumer.matches_source(source)
    };
    let published = match current {
        None => {
            if !leader.validate_fence(fence).await || !still_current() {
                return Err(ReconcileFailure::transient(None));
            }
            match secret_io::create(&publication, client).await {
                Ok(secret) => secret,
                Err(secret_io::SecretIoError::Kubernetes(kube::Error::Api(response))) if response.code == 409 => {
                    let winner = secret_io::get_required(client, &identity)
                        .await
                        .map_err(secret_io_failure)?;
                    if !secret_io::can_adopt_create_race(&winner, &identity) {
                        return Err(ReconcileFailure::rotation());
                    }
                    let cas = secret_io::capture_publication_cas(&winner, &identity).map_err(secret_io_failure)?;
                    if !leader.validate_fence(fence).await || !still_current() {
                        return Err(ReconcileFailure::transient(None));
                    }
                    secret_io::publish(client, &cas, &publication)
                        .await
                        .map_err(secret_io_failure)?
                }
                Err(error) => return Err(secret_io_failure(error)),
            }
        }
        Some(current) => {
            if current.immutable == Some(true) {
                if !secret_io::is_owned_by_consumer(&current, &identity) {
                    return Err(ReconcileFailure::rotation());
                }
                if !leader.validate_fence(fence).await || !still_current() {
                    return Err(ReconcileFailure::transient(None));
                }
                secret_io::delete_owned_consumer_object(client, &current, &identity)
                    .await
                    .map_err(secret_io_failure)?;
                if !leader.validate_fence(fence).await || !still_current() {
                    return Err(ReconcileFailure::transient(None));
                }
                let recreated = secret_io::create(&publication, client)
                    .await
                    .map_err(secret_io_failure)?;
                return secret_io::capture_delete_expectation(&recreated, identity, binding_revision)
                    .map_err(secret_io_failure);
            }
            if secret_io::classify_ownership(&current, &identity) == Ownership::Conflict {
                if !secret_io::is_owned_by_consumer(&current, &identity) {
                    return Err(ReconcileFailure::rotation());
                }
                if !leader.validate_fence(fence).await || !still_current() {
                    return Err(ReconcileFailure::transient(None));
                }
                return secret_io::repair_and_publish_owned_consumer(client, &current, &publication)
                    .await
                    .map_err(secret_io_failure)
                    .and_then(|published| {
                        secret_io::capture_delete_expectation(&published, identity, binding_revision)
                            .map_err(secret_io_failure)
                    });
            }
            let cas = secret_io::capture_publication_cas(&current, &identity).map_err(secret_io_failure)?;
            if !leader.validate_fence(fence).await || !still_current() {
                return Err(ReconcileFailure::transient(None));
            }
            secret_io::publish(client, &cas, &publication)
                .await
                .map_err(secret_io_failure)?
        }
    };
    secret_io::capture_delete_expectation(&published, identity, binding_revision).map_err(secret_io_failure)
}

async fn cleanup_removed_source(
    key: String,
    publications: HashMap<String, DeleteExpectation>,
    client: Client,
    leader: LeaderHandle,
    generation: u64,
) -> JobResult {
    let mut remaining = HashMap::new();
    if let Some(fence) = leader.current_fence() {
        let mut entries = publications.into_iter();
        while let Some((consumer_key, expectation)) = entries.next() {
            if !generation_and_fence_hold(generation, Some(&leader), Some(&fence)).await {
                remaining.insert(consumer_key, expectation);
                remaining.extend(entries);
                break;
            }
            if secret_io::delete_if_matches(&client, &expectation, || async {
                if !generation_and_fence_hold(generation, Some(&leader), Some(&fence)).await {
                    return Err(secret_io::SecretIoError::StaleOperation);
                }
                Ok(true)
            })
            .await
            .is_err()
            {
                remaining.insert(consumer_key, expectation);
            }
        }
    } else {
        remaining = publications;
    }
    JobResult::Cleaned { key, remaining }
}

async fn publish_manager_result(runtime: &ControllerRuntime, source: &DesiredCredentialSource, state: &SourceState) {
    let consumers = runtime
        .credential_source_registry
        .consumers_for_source(&source.logical_key());
    let published_consumer_count = if !source.persist && state.retained.is_some() {
        consumers
            .iter()
            .filter(|consumer| consumer.matches_source(source))
            .count() as u32
    } else {
        consumers
            .iter()
            .filter(|consumer| {
                state
                    .publications
                    .get(&consumer.logical_key())
                    .is_some_and(|expectation| {
                        consumer.matches_source(source)
                            && expectation.identity.consumer().uid == consumer.uid
                            && expectation.identity.source().uid == source.uid
                            && expectation.binding_revision == consumer.binding_revision()
                    })
            })
            .count() as u32
    };
    let now = SystemTime::now();
    let credential_state = if consumers.is_empty() {
        CredentialSourceState::Idle
    } else if state.retained.is_some() {
        CredentialSourceState::Valid
    } else if state.expired || state.last_valid_until.is_some_and(|deadline| deadline <= now) {
        CredentialSourceState::Expired
    } else if state.failure_count > 0 {
        CredentialSourceState::Unavailable
    } else {
        CredentialSourceState::Idle
    };
    let next_refresh_time = state.next_attempt.and_then(|deadline| {
        SystemTime::now()
            .checked_add(deadline.saturating_duration_since(Instant::now()))
            .map(format_rfc3339)
    });
    let result = CredentialManagerResult {
        observed_generation: source.generation,
        state: credential_state,
        published_generation: state.last_publication_generation.clone(),
        issued_at: state.last_issued_at.map(format_rfc3339),
        valid_until: state.last_valid_until.map(format_rfc3339),
        last_successful_refresh_time: state.last_issued_at.map(format_rfc3339),
        next_refresh_time,
        desired_consumer_count: consumers.len() as u32,
        published_consumer_count,
        conflicted_consumer_count: state.conflicted_consumer_count,
    };
    let key = source.logical_key();
    if source_is_current(runtime, source)
        && consumers
            .iter()
            .all(|consumer| consumer_is_current(runtime, consumer) && consumer.matches_source(source))
    {
        maybe_clear_memory_after_fan_out(runtime, source, published_consumer_count, consumers.len() as u32);
    }
    if runtime.credential_source_registry.replace_result(key.clone(), result) {
        if let Some(processor) = runtime.processor_registry.get("LinkSys") {
            processor.requeue(key).await;
        }
    }
}

fn source_is_current(runtime: &ControllerRuntime, expected: &DesiredCredentialSource) -> bool {
    let Some(current) = runtime.credential_source_registry.source(&expected.logical_key()) else {
        return false;
    };
    if current.uid != expected.uid
        || current.generation != expected.generation
        || source_input_revision(&current) != source_input_revision(expected)
    {
        return false;
    }
    let Some(processor) = runtime.processor_registry.get("LinkSys") else {
        return false;
    };
    let Some(raw) = processor.get_raw_any(&expected.logical_key()) else {
        return false;
    };
    let Ok(owner) = raw.downcast::<edgion_resources::resources::link_sys::LinkSys>() else {
        return false;
    };
    if owner.metadata.uid.as_deref() != Some(expected.uid.as_str())
        || owner.metadata.generation.unwrap_or_default() != expected.generation
        || expected.secret_inputs.is_empty()
    {
        return false;
    }
    let mut checked = Vec::new();
    for input in &expected.secret_inputs {
        if !checked
            .iter()
            .any(|prior: &&edgion_resources::resources::refs::FrozenFieldReference| {
                prior.same_owner_snapshot(&input.reference)
            })
        {
            if !input.reference.matches_link_sys(&owner) {
                return false;
            }
            checked.push(&input.reference);
        }
        let Ok(query) = input.reference.grant_query() else {
            return false;
        };
        let target = query.target();
        if target.group_kind().group() != ""
            || target.group_kind().kind() != "Secret"
            || target.scope().namespace() != Some(input.namespace.as_str())
            || target.name() != input.name
            || runtime.ref_grant_store.query_authorization_revision(query).as_deref()
                != Some(input.authorization_revision.as_str())
        {
            return false;
        }
        if !runtime
            .secret_store
            .get(Some(&input.namespace), &input.name)
            .is_some_and(|secret| {
                secret.metadata.uid == input.uid && secret.metadata.resource_version == input.resource_version
            })
        {
            return false;
        }
    }
    true
}

fn consumer_is_current(runtime: &ControllerRuntime, expected: &DesiredCredentialConsumer) -> bool {
    if expected.output_target().is_err() {
        return false;
    }
    if runtime
        .credential_source_registry
        .consumer(&expected.logical_key())
        .as_ref()
        != Some(expected)
    {
        return false;
    }
    let Some(processor) = runtime.processor_registry.get("EdgionPlugins") else {
        return false;
    };
    let Some(raw) = processor.get_raw_any(&expected.logical_key()) else {
        return false;
    };
    let Ok(owner) = raw.downcast::<edgion_resources::resources::edgion_plugins::EdgionPlugins>() else {
        return false;
    };
    if owner.metadata.uid.as_deref() != Some(expected.uid.as_str())
        || owner.metadata.generation.unwrap_or_default() != expected.generation
        || !expected.reference.matches_plugins(&owner)
    {
        return false;
    }
    let Ok(query) = expected.reference.grant_query() else {
        return false;
    };
    let target = query.target();
    target.group_kind().group() == "edgion.io"
        && target.group_kind().kind() == "LinkSys"
        && target
            .scope()
            .namespace()
            .is_some_and(|namespace| format!("{namespace}/{}", target.name()) == expected.source_key)
        && runtime.ref_grant_store.query_authorization_revision(query).as_deref()
            == Some(expected.authorization_revision.as_str())
}

fn reconciled_consumers_current(runtime: &ControllerRuntime, source_key: &str, expected_revision: &str) -> bool {
    let consumers = runtime.credential_source_registry.consumers_for_source(source_key);
    let Some(source) = runtime.credential_source_registry.source(source_key) else {
        return false;
    };
    consumer_revision(&consumers, 0) == expected_revision
        && source_is_current(runtime, &source)
        && consumers
            .iter()
            .all(|consumer| consumer_is_current(runtime, consumer) && consumer.matches_source(&source))
}

fn source_input_revision(source: &DesiredCredentialSource) -> String {
    let mut parts = source
        .secret_inputs
        .iter()
        .map(|input| {
            format!(
                "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
                input.reference.owner_revision(),
                input.reference.slot_revision(),
                input.role,
                input.namespace,
                input.name,
                input.uid.as_deref().unwrap_or_default(),
                input.resource_version.as_deref().unwrap_or_default(),
                input.authorization_revision
            )
        })
        .collect::<Vec<_>>();
    parts.push(format!("source-uid\0{}", source.uid));
    parts.push(format!("source-generation\0{}", source.generation));
    parts.sort();
    digest(parts.iter().map(String::as_str))
}

fn consumer_revision(consumers: &[DesiredCredentialConsumer], reconcile_epoch: u64) -> String {
    let mut parts = consumers
        .iter()
        .map(DesiredCredentialConsumer::binding_revision)
        .collect::<Vec<_>>();
    parts.sort();
    parts.push(format!("reconcile-epoch\0{reconcile_epoch}"));
    digest(parts.iter().map(String::as_str))
}

fn digest<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

#[cfg(test)]
#[path = "../../../tests/unit/services/credential_manager/service_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/services/credential_manager/service_test/tests_memory.rs"]
mod tests_memory;

#[cfg(test)]
#[path = "../../../tests/support/services/credential_manager/service.rs"]
mod test_support;
#[cfg(test)]
use test_support::*;

#[cfg(test)]
pub(crate) use test_support::lock_test_service;
