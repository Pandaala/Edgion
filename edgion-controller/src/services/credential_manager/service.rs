//! Restartable, leader-fenced credential acquisition service.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
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

use super::providers::oauth2::{BootstrapCredentials, OAuth2FailureKind, OAuth2Provider, OAuth2ProviderError};
use super::scheduler::{refresh_delay, retry_delay, usable_lifetime, EXPIRY_MARGIN, MIN_USABILITY};
use super::secret_io::{
    self, ConsumerIdentity, DeleteExpectation, ManagedSecretIdentity, Ownership, Publication, PublicationMetadata,
    SourceIdentity,
};
use edgion_resources::credentials::{BearerResolver, SecretMaterialResolver};
use edgion_resources::duration::parse_duration;
use edgion_resources::resources::link_sys::credential_source::CredentialSourceProvider;
use edgion_resources::resources::link_sys::CredentialSourceState;

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
    let mut service = SERVICE.lock();
    if service.is_some() {
        tracing::warn!(
            component = "credential_manager",
            "Credential manager already running; stop it before restarting"
        );
        return;
    }
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let task = tokio::spawn(run_worker(client, watch_namespaces, runtime, leader, generation));
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

#[derive(Default)]
struct SourceState {
    identity: String,
    input_revision: String,
    consumer_revision: String,
    next_attempt: Option<Instant>,
    failure_count: u32,
    expires_at: Option<Instant>,
    publications: HashMap<String, DeleteExpectation>,
    retained: Option<RetainedPublication>,
    publication_retry: bool,
    conflicted_consumer_count: u32,
    last_publication_generation: Option<String>,
    last_issued_at: Option<SystemTime>,
    last_valid_until: Option<SystemTime>,
}

#[derive(Clone)]
struct RetainedPublication {
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
        result: Result<ReconcileSuccess, ReconcileFailure>,
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
    retained: Option<RetainedPublication>,
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
        self.retained = Some(retained);
        self
    }
}

async fn run_worker(
    client: Client,
    watch_namespaces: Vec<String>,
    runtime: Arc<ControllerRuntime>,
    leader: LeaderHandle,
    generation: u64,
) {
    let mut sweep_complete = startup_sweep(&client, &watch_namespaces, &runtime, &leader, generation).await;
    let mut interval = tokio::time::interval(WAKE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut states = HashMap::<String, SourceState>::new();
    let mut in_flight = HashSet::<String>::new();
    let mut jobs = FuturesUnordered::<BoxFuture<'static, JobResult>>::new();

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if !is_current_generation(generation) || !leader.is_leader() {
                    continue;
                }
                if !sweep_complete {
                    sweep_complete = startup_sweep(
                        &client,
                        &watch_namespaces,
                        &runtime,
                        &leader,
                        generation,
                    )
                    .await;
                }
                let sources = runtime.credential_source_registry.sources();
                let source_keys: HashSet<_> = sources.iter().map(DesiredCredentialSource::logical_key).collect();

                for removed in states.keys().filter(|key| !source_keys.contains(*key)).cloned().collect::<Vec<_>>() {
                    if in_flight.contains(&removed) || jobs.len() >= GLOBAL_CONCURRENCY {
                        continue;
                    }
                    let old = states.get_mut(&removed).expect("key came from states");
                    if old.publications.is_empty() {
                        states.remove(&removed);
                        continue;
                    }
                    let publications = std::mem::take(&mut old.publications);
                    in_flight.insert(removed.clone());
                    jobs.push(Box::pin(cleanup_removed_source(
                        removed, publications, client.clone(), leader.clone(), generation,
                    )));
                }

                for source in sources {
                    if jobs.len() >= GLOBAL_CONCURRENCY {
                        break;
                    }
                    let key = source.logical_key();
                    if in_flight.contains(&key) {
                        continue;
                    }
                    let consumers = runtime.credential_source_registry.consumers_for_source(&key);
                    let identity = source.identity_key();
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
                        || state.consumer_revision != consumer_revision;
                    let expired = state.expires_at.is_some_and(|deadline| deadline <= Instant::now());
                    if expired {
                        if !state.publications.is_empty() && jobs.len() < GLOBAL_CONCURRENCY {
                            state.expires_at = None;
                            state.retained = None;
                            state.publication_retry = false;
                            let publications = std::mem::take(&mut state.publications);
                            in_flight.insert(key.clone());
                            jobs.push(Box::pin(cleanup_removed_source(
                                key, publications, client.clone(), leader.clone(), generation,
                            )));
                        } else if state.publications.is_empty() {
                            state.expires_at = None;
                            state.retained = None;
                            state.publication_retry = false;
                        }
                        continue;
                    }
                    let due = state.next_attempt.is_none_or(|deadline| deadline <= Instant::now());
                    if !(desired_changed || due) {
                        continue;
                    }
                    if consumers.is_empty() {
                        if !state.publications.is_empty() {
                            let publications = std::mem::take(&mut state.publications);
                            in_flight.insert(key.clone());
                            jobs.push(Box::pin(cleanup_removed_source(
                                key, publications, client.clone(), leader.clone(), generation,
                            )));
                        }
                        state.next_attempt = None;
                        state.expires_at = None;
                        state.retained = None;
                        state.publication_retry = false;
                        continue;
                    }
                    let previous_publications = state.publications.clone();
                    let retained = ((state.publication_retry || !due)
                        && state.identity == identity
                        && state.input_revision == input_revision)
                        .then(|| state.retained.clone())
                        .flatten();
                    in_flight.insert(key.clone());
                    jobs.push(Box::pin(reconcile_source(
                        source,
                        consumers,
                        previous_publications,
                        client.clone(),
                        runtime.clone(),
                        leader.clone(),
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
                            states.remove(&key);
                        }
                        if let (Some(source), Some(state)) = (
                            runtime.credential_source_registry.source(&key),
                            states.get(&key),
                        ) {
                            publish_manager_result(&runtime, &source, state).await;
                        }
                    }
                    JobResult::Reconciled { key, identity, input_revision, consumer_revision, result } => {
                        in_flight.remove(&key);
                        let Some(current) = runtime.credential_source_registry.source(&key) else {
                            continue;
                        };
                        if current.identity_key() != identity || source_input_revision(&current) != input_revision {
                            continue;
                        }
                        let state = states.entry(key).or_default();
                        state.identity = identity;
                        state.input_revision = input_revision;
                        state.consumer_revision = consumer_revision;
                        match result {
                            Ok(success) => {
                                state.failure_count = 0;
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
                                    state.expires_at = Some(retained.expires_at);
                                    state.retained = Some(retained);
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
    client: Client,
    runtime: Arc<ControllerRuntime>,
    leader: LeaderHandle,
    generation: u64,
    input_revision: String,
    consumer_revision: String,
    retained: Option<RetainedPublication>,
) -> JobResult {
    let key = source.logical_key();
    let identity = source.identity_key();
    let result = acquire_and_publish(
        &source,
        &consumers,
        previous_publications,
        &client,
        &runtime,
        &leader,
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
        result,
    }
}

async fn acquire_and_publish(
    source: &DesiredCredentialSource,
    consumers: &[DesiredCredentialConsumer],
    mut previous_publications: HashMap<String, DeleteExpectation>,
    client: &Client,
    runtime: &ControllerRuntime,
    leader: &LeaderHandle,
    generation: u64,
    input_revision: &str,
    retained: Option<RetainedPublication>,
) -> Result<ReconcileSuccess, ReconcileFailure> {
    let fence = leader
        .current_fence()
        .ok_or_else(|| ReconcileFailure::transient(None))?;
    if !is_current_generation(generation) || !source_is_current(runtime, source) || !leader.validate_fence(&fence).await
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
        let CredentialSourceProvider::OAuth2ClientCredentials(config) = &source.config.provider;
        let request_timeout =
            parse_duration(&source.config.rotation.request_timeout).map_err(|_| ReconcileFailure::rotation())?;
        let retry_cap =
            parse_duration(&source.config.rotation.retry_max_backoff).map_err(|_| ReconcileFailure::rotation())?;
        let resolver = crate::services::dns_resolver::service::controller_default_resolver(client).await;
        let provider = OAuth2Provider::new(
            config,
            request_timeout,
            retry_cap,
            resolver,
            source.config.egress.block_private,
        )
        .map_err(|_| ReconcileFailure::rotation())?;

        // The exact Lease tuple is re-read immediately before issuer I/O.
        if !is_current_generation(generation) || !leader.validate_fence(&fence).await {
            return Err(ReconcileFailure::transient(None));
        }
        let active_started_wall = SystemTime::now();
        let active_started_mono = Instant::now();
        let (token, request_started_wall, request_started_mono) = match provider.acquire(&active).await {
            Ok(token) => (token, active_started_wall, active_started_mono),
            Err(error) if error.kind == OAuth2FailureKind::InvalidClient && previous.is_some() => {
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

    for consumer in consumers {
        if !consumer_is_current(runtime, consumer)
            || !source_is_current(runtime, source)
            || !is_current_generation(generation)
        {
            return Err(ReconcileFailure::transient(None));
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
            &fence,
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

    // Bindings removed or switched while the old publication was live are
    // deleted only with the old source/binding/UID expectations.
    for (consumer_key, expectation) in previous_publications {
        if !is_current_generation(generation) || !leader.validate_fence(&fence).await {
            return Err(ReconcileFailure::transient(None));
        }
        let api = Api::<Secret>::namespaced(client.clone(), &expectation.identity.consumer.namespace);
        if secret_io::delete_if_matches(&api, &expectation).await.is_err() {
            publications.insert(consumer_key, expectation);
        }
    }

    Ok(ReconcileSuccess {
        next_delay,
        expires_at: Some(retained.expires_at),
        publications,
        retained,
    })
}

fn recover_source_state(
    runtime: &ControllerRuntime,
    source: &DesiredCredentialSource,
    consumers: &[DesiredCredentialConsumer],
    input_revision: &str,
) -> SourceState {
    let mut publication_ids = HashSet::new();
    let mut recovered: Option<RetainedPublication> = None;
    let mut publications = HashMap::new();

    for consumer in consumers {
        let Some(secret) = runtime
            .secret_store
            .get(Some(&consumer.target_secret_namespace), &consumer.target_secret_name)
        else {
            continue;
        };
        let identity = ManagedSecretIdentity {
            consumer: ConsumerIdentity {
                namespace: consumer.target_secret_namespace.clone(),
                name: consumer.target_secret_name.clone(),
                uid: consumer.uid.clone(),
                generation: consumer.generation,
            },
            source: SourceIdentity {
                name: source.logical_key(),
                uid: source.uid.clone(),
                generation: source.generation,
            },
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
            return SourceState::default();
        };
        let Ok(material) = BearerResolver.parse(data) else {
            return SourceState::default();
        };
        let Some(valid_until) = annotations
            .get(secret_io::ANNOTATION_VALID_UNTIL)
            .and_then(|value| parse_rfc3339_system_time(value))
        else {
            return SourceState::default();
        };
        let now_wall = SystemTime::now();
        let Some(remaining) = valid_until.duration_since(now_wall).ok() else {
            return SourceState::default();
        };
        if remaining < MIN_USABILITY {
            return SourceState::default();
        }
        let Some(expires_at) = Instant::now().checked_add(remaining) else {
            return SourceState::default();
        };
        let Some(issued_at) = annotations
            .get(secret_io::ANNOTATION_ISSUED_AT)
            .and_then(|value| parse_rfc3339_system_time(value))
        else {
            return SourceState::default();
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
            return SourceState::default();
        }
        recovered = Some(candidate);
        if let Some(binding_revision) = annotations.get(secret_io::ANNOTATION_BINDING_REVISION).cloned() {
            let Ok(expectation) = secret_io::capture_delete_expectation(&secret, identity, binding_revision) else {
                return SourceState::default();
            };
            publications.insert(consumer.logical_key(), expectation);
        }
    }

    if publication_ids.len() != 1 {
        return SourceState::default();
    }
    let Some(retained) = recovered else {
        return SourceState::default();
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
        publications,
        retained: Some(retained),
        publication_retry: false,
        conflicted_consumer_count: 0,
        last_publication_generation,
        last_issued_at,
        last_valid_until,
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
        let Some(expectation) = delete_expectation_from_secret(&secret) else {
            continue;
        };
        let consumer_key = format!(
            "{}/{}",
            expectation.identity.consumer.namespace, expectation.identity.consumer.name
        );
        let still_desired = runtime
            .credential_source_registry
            .consumer(&consumer_key)
            .is_some_and(|consumer| {
                consumer.uid == expectation.identity.consumer.uid
                    && consumer.source_uid == expectation.identity.source.uid
                    && consumer.source_generation == expectation.identity.source.generation
            });
        if still_desired {
            continue;
        }
        if !is_current_generation(generation) || !leader.validate_fence(&fence).await {
            return false;
        }
        let api = Api::<Secret>::namespaced(client.clone(), &expectation.identity.consumer.namespace);
        if secret_io::delete_if_matches(&api, &expectation).await.is_err() {
            tracing::warn!(
                component = "credential_manager",
                "Managed Secret startup cleanup failed"
            );
            return false;
        }
    }
    true
}

fn delete_expectation_from_secret(secret: &Secret) -> Option<DeleteExpectation> {
    let namespace = secret.metadata.namespace.clone()?;
    let annotations = secret.metadata.annotations.as_ref()?;
    let value = |key: &str| annotations.get(key).cloned();
    let consumer_name = value(secret_io::ANNOTATION_CONSUMER_NAME)?;
    if secret.metadata.name.as_deref() != Some(consumer_name.as_str()) {
        return None;
    }
    let identity = ManagedSecretIdentity {
        consumer: ConsumerIdentity {
            namespace,
            name: consumer_name,
            uid: value(secret_io::ANNOTATION_CONSUMER_UID)?,
            generation: value(secret_io::ANNOTATION_CONSUMER_GENERATION)?.parse().ok()?,
        },
        source: SourceIdentity {
            name: value(secret_io::ANNOTATION_SOURCE_NAME)?,
            uid: value(secret_io::ANNOTATION_SOURCE_UID)?,
            generation: value(secret_io::ANNOTATION_SOURCE_GENERATION)?.parse().ok()?,
        },
    };
    secret_io::capture_delete_expectation(secret, identity, value(secret_io::ANNOTATION_BINDING_REVISION)?).ok()
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
        secret_io::SecretIoError::Kubernetes(_) => ReconcileFailure::transient(None),
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
    let identity = ManagedSecretIdentity {
        consumer: ConsumerIdentity {
            namespace: consumer.target_secret_namespace.clone(),
            name: consumer.target_secret_name.clone(),
            uid: consumer.uid.clone(),
            generation: consumer.generation,
        },
        source: SourceIdentity {
            name: source.logical_key(),
            uid: source.uid.clone(),
            generation: source.generation,
        },
    };
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
    let api = Api::<Secret>::namespaced(client.clone(), &consumer.target_secret_namespace);
    let current = api
        .get_opt(&consumer.target_secret_name)
        .await
        .map_err(secret_io::SecretIoError::from)
        .map_err(secret_io_failure)?;
    let still_current = || {
        is_current_generation(generation)
            && source_is_current(runtime, source)
            && consumer_is_current(runtime, consumer)
    };
    let published = match current {
        None => {
            if !still_current() || !leader.validate_fence(fence).await {
                return Err(ReconcileFailure::transient(None));
            }
            match secret_io::create(&publication, &api).await {
                Ok(secret) => secret,
                Err(secret_io::SecretIoError::Kubernetes(kube::Error::Api(response))) if response.code == 409 => {
                    let winner = api
                        .get(&consumer.target_secret_name)
                        .await
                        .map_err(secret_io::SecretIoError::from)
                        .map_err(secret_io_failure)?;
                    if !secret_io::can_adopt_create_race(&winner, &identity) {
                        return Err(ReconcileFailure::rotation());
                    }
                    let cas = secret_io::capture_publication_cas(&winner, &identity).map_err(secret_io_failure)?;
                    if !still_current() || !leader.validate_fence(fence).await {
                        return Err(ReconcileFailure::transient(None));
                    }
                    secret_io::publish(&api, &cas, &publication)
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
                if !still_current() || !leader.validate_fence(fence).await {
                    return Err(ReconcileFailure::transient(None));
                }
                secret_io::delete_owned_consumer_object(&api, &current, &identity)
                    .await
                    .map_err(secret_io_failure)?;
                if !still_current() || !leader.validate_fence(fence).await {
                    return Err(ReconcileFailure::transient(None));
                }
                let recreated = secret_io::create(&publication, &api).await.map_err(secret_io_failure)?;
                return secret_io::capture_delete_expectation(&recreated, identity, binding_revision)
                    .map_err(secret_io_failure);
            }
            if secret_io::classify_ownership(&current, &identity) == Ownership::Conflict {
                if !secret_io::is_owned_by_consumer(&current, &identity) {
                    return Err(ReconcileFailure::rotation());
                }
                if !still_current() || !leader.validate_fence(fence).await {
                    return Err(ReconcileFailure::transient(None));
                }
                return secret_io::repair_and_publish_owned_consumer(&api, &current, &publication)
                    .await
                    .map_err(secret_io_failure)
                    .and_then(|published| {
                        secret_io::capture_delete_expectation(&published, identity, binding_revision)
                            .map_err(secret_io_failure)
                    });
            }
            let cas = secret_io::capture_publication_cas(&current, &identity).map_err(secret_io_failure)?;
            if !still_current() || !leader.validate_fence(fence).await {
                return Err(ReconcileFailure::transient(None));
            }
            secret_io::publish(&api, &cas, &publication)
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
            if !is_current_generation(generation) || !leader.validate_fence(&fence).await {
                remaining.insert(consumer_key, expectation);
                remaining.extend(entries);
                break;
            }
            let api = Api::<Secret>::namespaced(client.clone(), &expectation.identity.consumer.namespace);
            if secret_io::delete_if_matches(&api, &expectation).await.is_err() {
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
    let published_consumer_count = consumers
        .iter()
        .filter(|consumer| {
            state
                .publications
                .get(&consumer.logical_key())
                .is_some_and(|expectation| {
                    expectation.identity.consumer.uid == consumer.uid
                        && expectation.identity.source.uid == source.uid
                        && expectation.binding_revision == consumer.binding_revision()
                })
        })
        .count() as u32;
    let now = SystemTime::now();
    let credential_state = if consumers.is_empty() {
        CredentialSourceState::Idle
    } else if state.retained.is_some() {
        CredentialSourceState::Valid
    } else if state.last_valid_until.is_some_and(|deadline| deadline <= now) {
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
    if runtime.credential_source_registry.replace_result(key.clone(), result) {
        if let Some(processor) = runtime.processor_registry.get("LinkSys") {
            processor.requeue(key).await;
        }
    }
}

fn source_is_current(runtime: &ControllerRuntime, expected: &DesiredCredentialSource) -> bool {
    let registry_current = runtime
        .credential_source_registry
        .source(&expected.logical_key())
        .is_some_and(|current| {
            current.uid == expected.uid
                && current.generation == expected.generation
                && source_input_revision(&current) == source_input_revision(expected)
        });
    registry_current
        && expected.secret_inputs.iter().all(|input| {
            let source_namespace = expected.resource.namespace.as_deref().unwrap_or("default");
            let authority_current = runtime.ref_grant_store.authorization_revision(
                source_namespace,
                "edgion.io",
                "LinkSys",
                &input.namespace,
                "",
                "Secret",
                Some(&input.name),
            ) == input.authorization_revision;
            authority_current
                && runtime
                    .secret_store
                    .get(Some(&input.namespace), &input.name)
                    .is_some_and(|secret| {
                        secret.metadata.uid == input.uid && secret.metadata.resource_version == input.resource_version
                    })
        })
}

fn consumer_is_current(runtime: &ControllerRuntime, expected: &DesiredCredentialConsumer) -> bool {
    let desired_current = runtime
        .credential_source_registry
        .consumer(&expected.logical_key())
        .as_ref()
        == Some(expected);
    let consumer_namespace = expected.resource.namespace.as_deref().unwrap_or("default");
    let source_name = expected.source_key.rsplit('/').next().unwrap_or_default();
    desired_current
        && runtime.ref_grant_store.authorization_revision(
            consumer_namespace,
            "edgion.io",
            "EdgionPlugins",
            expected.source_key.split('/').next().unwrap_or("default"),
            "edgion.io",
            "LinkSys",
            Some(source_name),
        ) == expected.authorization_revision
}

fn source_input_revision(source: &DesiredCredentialSource) -> String {
    let mut parts = source
        .secret_inputs
        .iter()
        .map(|input| {
            format!(
                "{}\0{}\0{}\0{}\0{}\0{}",
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
mod tests {
    use super::*;
    use edgion_resources::ResourceKind;

    fn consumer(name: &str, generation: i64) -> DesiredCredentialConsumer {
        DesiredCredentialConsumer {
            resource: crate::conf_mgr::sync_runtime::resource_processor::ResourceRef::new(
                ResourceKind::EdgionPlugins,
                Some("apps".to_string()),
                name.to_string(),
            ),
            uid: format!("uid-{name}"),
            generation,
            source_key: "ops/source".to_string(),
            source_uid: "source-uid".to_string(),
            source_generation: 2,
            target_secret_namespace: "apps".to_string(),
            target_secret_name: name.to_string(),
            authorization_revision: "grant-rv".to_string(),
        }
    }

    #[test]
    fn consumer_set_revision_is_order_independent_and_change_sensitive() {
        let a = consumer("a", 1);
        let b = consumer("b", 1);
        assert_eq!(
            consumer_revision(&[a.clone(), b.clone()], 0),
            consumer_revision(&[b.clone(), a.clone()], 0)
        );
        assert_ne!(
            consumer_revision(&[a, b.clone()], 0),
            consumer_revision(&[consumer("a", 2), b], 0)
        );
        assert_ne!(consumer_revision(&[], 1), consumer_revision(&[], 2));
    }

    #[test]
    fn binding_revision_covers_consumer_and_source_generations() {
        let original = consumer("a", 1);
        let mut changed = original.clone();
        changed.source_generation += 1;
        assert_ne!(original.binding_revision(), changed.binding_revision());
    }

    #[test]
    fn recovery_never_restarts_rotation_interval_from_failover_time() {
        let issued = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let valid_until = issued + Duration::from_secs(3_570);
        let now_wall = issued + Duration::from_secs(15 * 60);
        let now_mono = Instant::now();
        let refresh = recovered_refresh_at(
            issued,
            valid_until,
            Duration::from_secs(15 * 60),
            "source/publication",
            now_wall,
            now_mono,
        );
        assert_eq!(refresh, now_mono, "passed periodic deadline must refresh immediately");
    }
}
