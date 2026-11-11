//! Controller DNS resolver for `controller`-mode `ExternalName` Services.
//!
//! Only Services annotated `edgion.io/dns-resolution-mode: controller` are
//! resolved here; every other `ExternalName` Service is resolved by the
//! Gateway. The work set is the [`DnsResolverTargets`] table that
//! `ServiceHandler` keeps in step with the processed Service cache.
//!
//! Answers are published as in-memory synthetic EndpointSlices
//! ([`DnsSyntheticSlices`]) and enqueued into the EndpointSlice processor,
//! which reads them through its synthetic-object hook and relays them to
//! Gateways over conf_sync. Nothing is written to the apiserver, so the
//! resolver works identically in Kubernetes, file-system and etcd modes, and
//! it runs in every process that runs the resource pipeline (Kubernetes
//! all-serve replicas included). Its lifecycle belongs to the
//! `ControllerRuntime`: started with the first target, aborted by `reset()`.
//!
//! Refresh is a fixed `refresh_interval`; record TTLs are deliberately not
//! honored (see
//! `skills/04-review/controller/intentional-design/dns-resolver-fixed-interval-refresh.md`).
//! An unchanged answer publishes nothing. The first pass waits until the
//! resource pipeline is ready, so httpdns LinkSys and ReferenceGrant state are
//! known before any answer is published. Published endpoints live only in
//! this process: after a Controller restart or leadership change a
//! `controller`-mode Service has endpoints again once its first answer lands.
//!
//! [`DnsResolverTargets`]: super::targets::DnsResolverTargets
//! [`DnsSyntheticSlices`]: super::synthetic::DnsSyntheticSlices

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::core::v1::Service;
use parking_lot::Mutex;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::conf_mgr::sync_runtime::resource_processor::handlers::dns_resolver_guard::dns_query_allowed;
use crate::conf_mgr::ControllerRuntime;
use edgion_common::dns::endpoint_slices::{build_dns_endpoint_slices, normalize_addrs, DnsSliceOrigin};
use edgion_common::dns::resolver_select::{
    parse_dns_server_list, resolve_effective_spec, LinkSysKey, ResolverCandidate,
};
use edgion_common::dns::{
    resolve, DnsError, DnsLookupFamily, DnsQueryPolicy, HttpDnsCandidate, PreparedHttpDns, ResolverSpec,
    StandardDnsServers,
};
use edgion_resources::constants::annotations::edgion::{DNS_LOOKUP_FAMILY, DNS_RESOLVER as DNS_RESOLVER_ANNOTATION};
use edgion_resources::resources::link_sys::LinkSys;
use edgion_resources::resources::refs::dns_fields::DnsResolverOwner;

use crate::config::{dns_resolver_config, DnsResolverConfig};

use super::synthetic::{DnsSyntheticSlices, Provenance};
use super::targets::DnsResolverTarget;

/// Fallback refresh interval when the configured `refresh_interval` is unparseable.
const DEFAULT_REFRESH: Duration = Duration::from_secs(15);

/// Lower bound on the refresh interval and on the spacing between passes.
const MIN_REFRESH: Duration = Duration::from_secs(1);

/// Upper bound on concurrent DNS queries within one pass.
const MAX_CONCURRENT_RESOLUTIONS: usize = 8;

/// Per-query deadline, so one unreachable resolver cannot hold a pass open.
const RESOLVE_DEADLINE: Duration = Duration::from_secs(30);

/// Longest a last good answer is kept while resolution keeps failing
/// transiently. The OS resolver reports NXDOMAIN as a transient error, so this
/// is also how long a deleted name keeps its endpoints.
const MAX_STALE: Duration = Duration::from_secs(300);

/// Poll interval while waiting for the resource pipeline to become ready.
const READY_POLL: Duration = Duration::from_millis(200);

/// Delay before a panicked resolver task is restarted.
const PANIC_RESTART_DELAY: Duration = Duration::from_secs(1);

/// The EndpointSlice processor kind that consumes the synthetic slices.
const ENDPOINT_SLICE_KIND: &str = "EndpointSlice";

/// Handle of the resolver task owned by one `ControllerRuntime`.
#[derive(Default)]
pub(crate) struct DnsResolverTask {
    abort: Mutex<Option<tokio::task::AbortHandle>>,
}

impl DnsResolverTask {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Abort the task (runtime reset). The synthetic store is cleared by the caller.
    pub(crate) fn stop(&self) {
        if let Some(abort) = self.abort.lock().take() {
            abort.abort();
            tracing::info!(component = "dns_resolver_service", "DNS resolver stopped");
        }
    }

    /// The task was started and has not finished or been stopped.
    pub(crate) fn is_running(&self) -> bool {
        self.abort.lock().as_ref().is_some_and(|abort| !abort.is_finished())
    }
}

/// Start the resolver for `runtime` when it has targets (or published slices
/// still to withdraw) and is not running. Called by `ServiceHandler` after
/// publishing a Service and by the panic watcher; idempotent.
pub(crate) fn ensure_dns_resolver_running(runtime: &Arc<ControllerRuntime>) {
    let idle = runtime.dns_resolver_targets.is_empty() && runtime.dns_synthetic_slices.is_empty();
    if idle || runtime.dns_resolver_task.is_running() {
        return;
    }
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let mut guard = runtime.dns_resolver_task.abort.lock();
    if guard.as_ref().is_some_and(|abort| !abort.is_finished()) {
        return;
    }
    let task = tokio::spawn(run(Arc::clone(runtime)));
    *guard = Some(task.abort_handle());
    drop(guard);
    // An abort (runtime reset) is silent. A panic is reported and the task is
    // restarted after a short delay, even when the last target left meanwhile,
    // so the dead task's slices are withdrawn (a reset in between clears both
    // targets and slices, so the restart is then a no-op).
    let runtime = Arc::clone(runtime);
    tokio::spawn(async move {
        if let Err(error) = task.await {
            if !error.is_cancelled() {
                tracing::error!(
                    component = "dns_resolver_service",
                    "DNS resolver task panicked; restarting"
                );
                tokio::time::sleep(PANIC_RESTART_DELAY).await;
                ensure_dns_resolver_running(&runtime);
            }
        }
    });
    tracing::info!(component = "dns_resolver_service", "DNS resolver started");
}

/// Per-Service resolution state.
#[derive(Default)]
pub(crate) struct TargetState {
    revision: Option<u64>,
    /// The Service content of `revision` (ports for publication).
    service: Option<Arc<Service>>,
    next_due: Option<Instant>,
    /// A query for `revision` is running.
    in_flight: bool,
    /// Last good normalized answer. The query already applied that pass's
    /// family, so a narrowed family filters it at once while a widened one
    /// gains addresses only with the next answer.
    addrs: Option<Arc<[IpAddr]>>,
    /// Hostname and resolver fingerprint that produced `addrs`; a different
    /// source never inherits them.
    source: Option<(String, String)>,
    last_success: Option<Instant>,
    prepared: Option<(LinkSysKey, Arc<PreparedHttpDns>)>,
}

impl TargetState {
    fn clear_answer(&mut self) {
        self.addrs = None;
        self.source = None;
        self.last_success = None;
    }

    fn prepare(&mut self, key: &LinkSysKey, owner: Arc<LinkSys>) -> Option<Arc<PreparedHttpDns>> {
        let Some(incoming) = HttpDnsCandidate::from_link_sys(owner) else {
            self.prepared = None;
            return None;
        };
        if let Some((previous_key, prepared)) = &self.prepared {
            if previous_key == key && incoming.matches(prepared) {
                return Some(Arc::clone(prepared));
            }
        }
        let prepared = incoming.prepare();
        // Kept even on failure so refreshes do not rebuild an invalid generation
        // or repeat its TLS audit every tick.
        self.prepared = Some((key.clone(), Arc::clone(&prepared)));
        Some(prepared)
    }
}

/// One DNS query of a pass.
pub(crate) struct ResolveJob {
    pub(crate) service_key: String,
    /// Target revision the query was planned for; a result for an older
    /// revision is discarded.
    pub(crate) revision: u64,
    pub(crate) hostname: String,
    pub(crate) spec: ResolverSpec,
    pub(crate) family: DnsLookupFamily,
}

/// Resolver state across passes: the pure-ish core driven by [`run`].
pub(crate) struct ResolverState {
    /// Synthetic-store epoch this task writes under (see `DnsSyntheticSlices::clear`).
    epoch: u64,
    period: Duration,
    default_candidate: Option<ResolverCandidate>,
    default_family: DnsLookupFamily,
    targets: HashMap<String, TargetState>,
}

impl ResolverState {
    pub(crate) fn new(config: &DnsResolverConfig, epoch: u64) -> Self {
        Self {
            epoch,
            period: parse_refresh_interval(&config.refresh_interval),
            default_candidate: controller_default_candidate(config),
            default_family: config.lookup_family,
            targets: HashMap::new(),
        }
    }

    /// Reconcile with the target table. Withdraws removed targets and answers
    /// older than `MAX_STALE`, republishes changed targets from their last
    /// good answer (new ports, narrowed family), and returns the due queries
    /// plus the slice keys to enqueue.
    pub(crate) fn plan(
        &mut self,
        targets: Vec<(String, DnsResolverTarget)>,
        runtime: &ControllerRuntime,
        slices: &DnsSyntheticSlices,
        now: Instant,
    ) -> (Vec<ResolveJob>, Vec<String>) {
        let mut changed_keys = Vec::new();
        let live: HashSet<&str> = targets.iter().map(|(key, _)| key.as_str()).collect();
        let removed: Vec<String> = self
            .targets
            .keys()
            .filter(|key| !live.contains(key.as_str()))
            .cloned()
            .collect();
        for key in removed {
            self.targets.remove(&key);
            changed_keys.extend(slices.replace_service(self.epoch, &key, Vec::new()));
        }
        // Slices whose target vanished while no state existed (for example a
        // restarted loop) are withdrawn as well.
        for key in slices.service_keys() {
            if !live.contains(key.as_str()) {
                changed_keys.extend(slices.replace_service(self.epoch, &key, Vec::new()));
            }
        }

        // Targets due within this window join the pass instead of waking it again.
        let horizon = now + MIN_REFRESH;
        let mut jobs = Vec::new();
        for (key, target) in targets {
            let is_new = !self.targets.contains_key(&key);
            let stale_limit = self.stale_limit();
            let state = self.targets.entry(key.clone()).or_default();
            // The stale bound holds while a refresh still waits for a permit
            // or runs: an answer that old is withdrawn at this pass (the wake
            // time includes the deadline), not when the query completes.
            if state
                .last_success
                .is_some_and(|last| now.saturating_duration_since(last) >= stale_limit)
            {
                state.addrs = None;
                state.last_success = None;
                changed_keys.extend(slices.replace_service(self.epoch, &key, Vec::new()));
            }
            let revised = state.revision != Some(target.revision);
            if !revised && (state.in_flight || state.next_due.is_some_and(|due| due > horizon)) {
                continue;
            }
            state.revision = Some(target.revision);
            state.service = Some(Arc::clone(&target.service));
            state.next_due = Some(horizon + self.period);
            let service = target.service;
            let family = match service_lookup_family(&service, self.default_family) {
                Ok(family) => family,
                Err(error) => {
                    tracing::warn!(
                        component = "dns_resolver_service",
                        service = %key,
                        error,
                        "Invalid DNS family policy; withdrawing Controller-published endpoints"
                    );
                    state.clear_answer();
                    changed_keys.extend(slices.replace_service(self.epoch, &key, Vec::new()));
                    continue;
                }
            };
            let Some(hostname) = external_name(&service) else {
                state.clear_answer();
                changed_keys.extend(slices.replace_service(self.epoch, &key, Vec::new()));
                continue;
            };
            let candidate = service_resolver_candidate(&service, runtime);
            let spec = effective_spec(runtime, candidate, &self.default_candidate, state);
            let source = (hostname.clone(), spec.fingerprint().into_owned());
            if state.source.as_ref() != Some(&source) {
                // A new hostname or resolver never inherits the old answer. A
                // fresh state (first pass, or a restarted task) adopts the
                // source and leaves what is published until its first answer.
                if !is_new {
                    state.clear_answer();
                    changed_keys.extend(slices.replace_service(self.epoch, &key, Vec::new()));
                } else if slices.has_service(&key) {
                    match slices.provenance(&key) {
                        // Adopt what an earlier task published for the same
                        // source, keeping its answer time so the stale bound
                        // still counts from the original answer. An edit made
                        // while no task ran (ports, family) applies now.
                        Some(published)
                            if published.source == source
                                && now.saturating_duration_since(published.answered_at) < stale_limit =>
                        {
                            state.last_success = Some(published.answered_at);
                            state.addrs = Some(published.addrs);
                            state.source = Some(source.clone());
                            changed_keys.extend(publish(
                                slices,
                                self.epoch,
                                &key,
                                &service,
                                state.addrs.as_deref(),
                                family,
                                provenance(state),
                            ));
                        }
                        // Published for another hostname or resolver (or of
                        // unknown origin): never served for this source.
                        _ => changed_keys.extend(slices.replace_service(self.epoch, &key, Vec::new())),
                    }
                }
                state.source = Some(source);
            } else if revised && state.addrs.is_some() {
                // Same source, new ports or family: republish the last good answer now.
                changed_keys.extend(publish(
                    slices,
                    self.epoch,
                    &key,
                    &service,
                    state.addrs.as_deref(),
                    family,
                    provenance(state),
                ));
            }
            state.in_flight = true;
            jobs.push(ResolveJob {
                service_key: key,
                revision: target.revision,
                hostname,
                spec,
                family,
            });
        }
        (jobs, changed_keys)
    }

    /// Apply one answer. Success replaces the set, an authoritative empty
    /// answer withdraws it, any other failure keeps the last good set for at
    /// most `stale_limit` (normally `MAX_STALE`). A result for a removed or since-revised target is
    /// discarded.
    pub(crate) fn apply(
        &mut self,
        job: &ResolveJob,
        result: Result<Vec<IpAddr>, DnsError>,
        slices: &DnsSyntheticSlices,
        now: Instant,
    ) -> Vec<String> {
        let stale_limit = self.stale_limit();
        let Some(state) = self.targets.get_mut(&job.service_key) else {
            return Vec::new();
        };
        if state.revision != Some(job.revision) {
            return Vec::new();
        }
        state.in_flight = false;
        // The next refresh counts from completion: a slow query (up to the
        // resolve deadline) never makes the target overdue on arrival.
        state.next_due = Some(now + self.period);
        match result {
            Ok(addrs) if !addrs.is_empty() => {
                let normalized = normalize_addrs(addrs);
                state.addrs = (!normalized.is_empty()).then(|| Arc::from(normalized));
                state.last_success = Some(now);
            }
            Ok(_) | Err(DnsError::NoAddresses) => state.addrs = None,
            Err(error) => {
                let fresh = state
                    .last_success
                    .is_some_and(|last| now.saturating_duration_since(last) < stale_limit);
                tracing::warn!(
                    component = "dns_resolver_service",
                    service = %job.service_key,
                    external_name = %job.hostname,
                    error = %error,
                    keep_last_good = fresh,
                    "DNS resolution failed"
                );
                if fresh {
                    return Vec::new();
                }
                state.addrs = None;
            }
        }
        let Some(service) = state.service.clone() else {
            return Vec::new();
        };
        publish(
            slices,
            self.epoch,
            &job.service_key,
            &service,
            state.addrs.as_deref(),
            job.family,
            provenance(state),
        )
    }

    /// How long a last good answer may go without a successful refresh:
    /// `MAX_STALE`, or one refresh period plus one full query when that is
    /// longer, so a healthy target with a long `refresh_interval` is never
    /// withdrawn while its next refresh is simply not due yet.
    fn stale_limit(&self) -> Duration {
        MAX_STALE.max(self.period.saturating_add(MIN_REFRESH + RESOLVE_DEADLINE))
    }

    /// Earliest due refresh or stale deadline. Targets with a query in flight
    /// are rescheduled when it completes, so their refresh never pulls the
    /// wake time into the past; a stale deadline is cleared by the pass it
    /// wakes.
    pub(crate) fn next_due(&self, now: Instant) -> Instant {
        let stale_limit = self.stale_limit();
        let refreshes = self
            .targets
            .values()
            .filter(|state| !state.in_flight)
            .filter_map(|state| state.next_due);
        let deadlines = self
            .targets
            .values()
            .filter(|state| state.addrs.is_some())
            .filter_map(|state| state.last_success?.checked_add(stale_limit));
        refreshes.chain(deadlines).min().unwrap_or(now + self.period)
    }
}

/// What produced a target's current answer, recorded with its slices.
fn provenance(state: &TargetState) -> Option<Provenance> {
    Some(Provenance {
        source: state.source.clone()?,
        answered_at: state.last_success?,
        addrs: state.addrs.clone()?,
    })
}

fn external_name(service: &Service) -> Option<String> {
    service
        .spec
        .as_ref()
        .filter(|spec| spec.type_.as_deref() == Some("ExternalName"))
        .and_then(|spec| spec.external_name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// Rebuild one Service's synthetic slices from an answer (or withdraw them).
/// Returns the slice keys that changed; an unchanged answer returns none.
fn publish(
    slices: &DnsSyntheticSlices,
    epoch: u64,
    service_key: &str,
    service: &Service,
    addrs: Option<&[IpAddr]>,
    family: DnsLookupFamily,
    provenance: Option<Provenance>,
) -> Vec<String> {
    let allowed: Vec<IpAddr> = addrs
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|ip| family.allows(*ip))
        .collect();
    let built = match service_key.split_once('/') {
        Some((namespace, name)) => build_dns_endpoint_slices(
            namespace,
            name,
            service
                .spec
                .as_ref()
                .and_then(|spec| spec.ports.as_deref())
                .unwrap_or(&[]),
            &allowed,
            DnsSliceOrigin::Controller,
        ),
        None => Vec::new(),
    };
    slices.replace_service_from(epoch, service_key, provenance, built)
}

/// Wait until every resource processor is registered and ready, so httpdns
/// LinkSys owners and ReferenceGrants are known before the first answer.
async fn wait_until_ready(runtime: &ControllerRuntime) {
    let registry = &runtime.processor_registry;
    while !(registry.is_registration_complete() && registry.is_all_ready()) {
        tokio::time::sleep(READY_POLL).await;
    }
}

/// The loop body. Wakes when a target is due, the target table changes or a
/// query completes; queries run concurrently in the background, so target
/// changes are planned while earlier queries are still in flight.
async fn run(runtime: Arc<ControllerRuntime>) {
    wait_until_ready(&runtime).await;
    let config = dns_resolver_config();
    // Captured before the first write: a reset after this point makes every
    // write of this (aborting) task a no-op.
    let mut state = ResolverState::new(&config, runtime.dns_synthetic_slices.epoch());
    tracing::info!(
        component = "dns_resolver_service",
        interval_secs = state.period.as_secs(),
        "DNS resolver loop running"
    );
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_RESOLUTIONS));
    let mut queries: JoinSet<(ResolveJob, Result<Vec<IpAddr>, DnsError>)> = JoinSet::new();
    let mut next_wake = Instant::now();
    let mut last_pass: Option<Instant> = None;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(next_wake) => {}
            _ = runtime.dns_resolver_targets.changed() => {}
            Some(done) = queries.join_next(), if !queries.is_empty() => {
                if let Ok((job, result)) = done {
                    let keys = state.apply(&job, result, &runtime.dns_synthetic_slices, Instant::now());
                    enqueue(&runtime, keys).await;
                }
                next_wake = state.next_due(Instant::now());
                continue;
            }
        }
        // Coalesce bursts of target changes (for example the initial cache load).
        if let Some(last) = last_pass {
            tokio::time::sleep_until(last + MIN_REFRESH).await;
        }
        let now = Instant::now();
        last_pass = Some(now);

        let targets = runtime.dns_resolver_targets.snapshot();
        let (jobs, changed) = state.plan(targets, &runtime, &runtime.dns_synthetic_slices, now);
        enqueue(&runtime, changed).await;
        for job in jobs {
            let permits = Arc::clone(&permits);
            queries.spawn(async move {
                let _permit = permits.acquire_owned().await;
                let result = query(&job).await;
                (job, result)
            });
        }
        next_wake = state.next_due(Instant::now());
    }
}

async fn enqueue(runtime: &ControllerRuntime, keys: Vec<String>) {
    for key in keys {
        runtime.processor_registry.requeue(ENDPOINT_SLICE_KIND, key).await;
    }
}

/// One bounded query. It runs in its own task so a panicking resolver backend
/// becomes an ordinary failure and never leaves the target in flight.
async fn query(job: &ResolveJob) -> Result<Vec<IpAddr>, DnsError> {
    let spec = job.spec.clone();
    let hostname = job.hostname.clone();
    let policy = DnsQueryPolicy::Family(job.family);
    let lookup =
        tokio::spawn(async move { tokio::time::timeout(RESOLVE_DEADLINE, resolve(&spec, &hostname, policy)).await });
    match lookup.await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(DnsError::Io("resolution deadline exceeded".to_string())),
        Err(_) => Err(DnsError::Io("resolution task failed".to_string())),
    }
}

fn service_lookup_family(service: &Service, default: DnsLookupFamily) -> Result<DnsLookupFamily, &'static str> {
    DnsLookupFamily::from_annotation(
        service
            .metadata
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(DNS_LOOKUP_FAMILY))
            .map(String::as_str),
    )
    .map(|family| family.unwrap_or(default))
}

/// Tenant annotation permission is distinct from administrative default provenance.
fn service_resolver_candidate(service: &Service, runtime: &ControllerRuntime) -> Option<ResolverCandidate> {
    if let Ok(Some(query)) = DnsResolverOwner::Service(service).query() {
        if dns_query_allowed(query, runtime) {
            return Some(ResolverCandidate::HttpDnsRef(LinkSysKey {
                namespace: query.target().scope().namespace()?.to_string(),
                name: query.target().name().to_string(),
            }));
        }
    }
    let servers = service.metadata.annotations.as_ref()?.get(DNS_RESOLVER_ANNOTATION)?;
    let servers = parse_dns_server_list(servers);
    (!servers.is_empty()).then(|| ResolverCandidate::Servers(StandardDnsServers::new(servers)))
}

/// Control-plane effective resolver: Service annotation -> Controller-YAML
/// default -> system (no Gateway / EGC layers). httpdns refs resolve against
/// the processed LinkSys cache, which carries the Controller-resolved TLS material.
fn effective_spec(
    runtime: &ControllerRuntime,
    service_candidate: Option<ResolverCandidate>,
    default_candidate: &Option<ResolverCandidate>,
    state: &mut TargetState,
) -> ResolverSpec {
    let layers = [service_candidate.as_ref(), default_candidate.as_ref()];
    let prepared = match layers.iter().flatten().next() {
        Some(ResolverCandidate::HttpDnsRef(key)) => match processed_httpdns_owner(runtime, key) {
            Some(owner) => state.prepare(key, owner),
            None => {
                state.prepared = None;
                None
            }
        },
        _ => {
            state.prepared = None;
            None
        }
    };
    resolve_effective_spec(&layers, |_| prepared.clone())
}

/// Resolve the Controller-wide default through the same precedence and LinkSys
/// lookup used for `controller`-mode Services.
pub(crate) fn controller_default_resolver(runtime: &ControllerRuntime) -> ResolverSpec {
    let config = dns_resolver_config();
    let default_candidate = controller_default_candidate(&config);
    effective_spec(runtime, None, &default_candidate, &mut TargetState::default())
}

/// Look up a processed httpdns LinkSys owner. `None` when absent or not an
/// httpdns LinkSys (a dangling ref then fails safe to the system resolver).
fn processed_httpdns_owner(runtime: &ControllerRuntime, key: &LinkSysKey) -> Option<Arc<LinkSys>> {
    let cached = runtime
        .processor_registry
        .get("LinkSys")?
        .get_cached_any(&format!("{}/{}", key.namespace, key.name))?;
    let owner = cached.downcast::<LinkSys>().ok()?;
    owner.spec.config.as_http_dns().is_some().then_some(owner)
}

/// Controller default resolver candidate: `link_sys_ref` (httpdns) wins over
/// `servers`; the two are validated mutually exclusive at config load.
fn controller_default_candidate(config: &DnsResolverConfig) -> Option<ResolverCandidate> {
    if let Some(reference) = config.link_sys_ref.as_deref() {
        if let Some(key) = LinkSysKey::parse(reference) {
            return Some(ResolverCandidate::HttpDnsRef(key));
        }
    }
    if !config.servers.is_empty() {
        return Some(ResolverCandidate::Servers(StandardDnsServers::new(
            config.servers.clone(),
        )));
    }
    None
}

/// Parse a `refresh_interval` duration string. Unparseable values fall back to
/// [`DEFAULT_REFRESH`]; anything below [`MIN_REFRESH`] is clamped up.
fn parse_refresh_interval(value: &str) -> Duration {
    parse_duration_str(value).unwrap_or(DEFAULT_REFRESH).max(MIN_REFRESH)
}

/// Parse a GEP-2257 duration string through the canonical
/// [`edgion_resources::duration::parse_duration`]. Shared with
/// `DnsResolverConfig::validate` for load-time rejection.
pub(crate) fn parse_duration_str(value: &str) -> Option<Duration> {
    edgion_resources::duration::parse_duration(value).ok()
}

#[cfg(test)]
#[path = "../../../tests/unit/services/dns_resolver/service_test.rs"]
mod tests;
