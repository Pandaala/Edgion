//! Control-plane DNS resolver service.
//!
//! A leader-only background loop (started alongside the ACME service, K8s
//! conf_center mode only) that resolves `mode=controller` ExternalName Services
//! via their effective resolver and writes a **real** `EndpointSlice` to the
//! apiserver. Its work set is the
//! [`DnsResolverTargets`](super::targets::DnsResolverTargets) table maintained by
//! `ServiceHandler` from the processed Service cache, and httpdns owners come
//! from the processed LinkSys cache, so the loop never lists Services or reads
//! LinkSys objects from the apiserver. The published slice flows to the gateway through the
//! existing reflector → conf_sync → EndpointSlice-LB path, giving the
//! "control-plane resolves centrally, multi-address LB" behavior (Envoy
//! STRICT_DNS-like).
//!
//! In FileSystem/Etcd conf_center modes this service is never started; there,
//! `mode=controller` Services degrade to the gateway self-resolution bridge.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::StreamExt;
use k8s_openapi::api::core::v1::{Service, ServicePort};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, DeleteParams, ListParams, ObjectMeta, Patch, PatchParams, Preconditions};
use kube::Client;
use parking_lot::Mutex;

use crate::conf_mgr::sync_runtime::resource_processor::handlers::dns_resolver_guard::dns_query_allowed;
use crate::conf_mgr::ControllerRuntime;
use edgion_common::dns::resolver_select::{
    parse_dns_server_list, resolve_effective_spec, LinkSysKey, ResolverCandidate,
};
use edgion_common::dns::{
    resolve, DnsLookupFamily, DnsQueryPolicy, HttpDnsCandidate, PreparedHttpDns, ResolverSpec, StandardDnsServers,
};
use edgion_common::time::now_rfc3339;
use edgion_resources::constants::annotations::edgion::{DNS_LOOKUP_FAMILY, DNS_RESOLVER as DNS_RESOLVER_ANNOTATION};
use edgion_resources::constants::labels::dns::{MANAGED_BY_VALUE, SYNTHETIC};
use edgion_resources::constants::labels::k8s::{ENDPOINTSLICE_MANAGED_BY, SERVICE_NAME};
use edgion_resources::resources::link_sys::LinkSys;
use edgion_resources::resources::refs::dns_fields::DnsResolverOwner;

use crate::config::{dns_resolver_config, DnsResolverConfig};

use super::slice_builder::{build_endpoint_slices, endpoint_ports, managed_by_selector, slice_name, FIELD_MANAGER};

/// Fallback tick interval when the configured `refresh_interval` is unparseable.
const DEFAULT_REFRESH: Duration = Duration::from_secs(15);

/// Lower bound on the refresh interval and on the spacing between passes. A tiny
/// interval would turn the loop into a resolve + patch storm, so anything below
/// this is clamped up. Also enforced at config load (`DnsResolverConfig::validate`).
const MIN_REFRESH: Duration = Duration::from_secs(1);

/// Upper bound on concurrent DNS queries within one pass.
const MAX_CONCURRENT_RESOLUTIONS: usize = 8;

/// Per-query deadline, so one unreachable resolver cannot hold a pass open.
/// Resolver-level timeouts normally fire first.
const RESOLVE_DEADLINE: Duration = Duration::from_secs(30);

/// Handle to the running resolver loop. Stores an `AbortHandle` (not the
/// `JoinHandle` — the supervision observer owns that, and `JoinHandle` is not
/// cloneable) plus a generation so a stale observer can recognize a restart.
struct DnsResolverHandle {
    abort: tokio::task::AbortHandle,
    generation: u64,
}

/// Global singleton handle (mirrors the leader-only ACME service shape).
static DNS_RESOLVER: Mutex<Option<DnsResolverHandle>> = Mutex::new(None);

/// Monotonic generation counter; first running generation is 1.
static DNS_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Start the control-plane DNS resolver loop. Idempotent: a warning + no-op if
/// already running (call [`stop_dns_resolver_service`] first on re-election).
pub fn start_dns_resolver_service(client: Client, watch_namespaces: Vec<String>, runtime: Arc<ControllerRuntime>) {
    use std::sync::atomic::Ordering;

    let mut guard = DNS_RESOLVER.lock();
    if guard.is_some() {
        tracing::warn!(
            component = "dns_resolver_service",
            "DNS resolver service already running, call stop_dns_resolver_service() first"
        );
        return;
    }
    let generation = DNS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = tokio::spawn(async move { run(client, watch_namespaces, runtime).await });
    *guard = Some(DnsResolverHandle {
        abort: handle.abort_handle(),
        generation,
    });
    drop(guard);

    crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("dns_resolver").set(1.0);
    crate::services::supervision::observe_worker("dns_resolver", handle, move || {
        let guard = DNS_RESOLVER.lock();
        let current = guard.as_ref().map(|h| h.generation) == Some(generation);
        if current {
            crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("dns_resolver").set(0.0);
        }
        current
    });
    tracing::info!(component = "dns_resolver_service", "DNS resolver service started");
}

/// Stop the loop (leadership loss / shutdown). Aborts the background task; the
/// observer sees `JoinError::is_cancelled()` and classifies it orderly.
pub fn stop_dns_resolver_service() {
    if let Some(handle) = DNS_RESOLVER.lock().take() {
        handle.abort.abort();
        crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("dns_resolver").set(0.0);
        tracing::info!(component = "dns_resolver_service", "DNS resolver service stopped");
    }
}

/// Per-Service ownership keeps preparation independent of EndpointSlice writes.
#[derive(Default)]
struct ServiceDnsState {
    applied_fingerprint: Option<String>,
    prepared: Option<(LinkSysKey, Arc<PreparedHttpDns>)>,
    /// Target revision handled by the last pass; a different revision is due now.
    target_revision: Option<u64>,
    /// Next scheduled refresh.
    next_due: Option<tokio::time::Instant>,
}

impl ServiceDnsState {
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
        // Store before the query, including failure, so refreshes do not rebuild
        // invalid generations or repeat their TLS audit every tick.
        self.prepared = Some((key.clone(), Arc::clone(&prepared)));
        Some(prepared)
    }
}

/// The loop body. Wakes when a target is due or the target table changes.
async fn run(client: Client, watch_namespaces: Vec<String>, runtime: Arc<ControllerRuntime>) {
    // Controller configuration is loaded once at process startup.
    let config = dns_resolver_config();
    let period = parse_refresh_interval(&config.refresh_interval);
    tracing::info!(
        component = "dns_resolver_service",
        enabled = config.enabled,
        interval_secs = period.as_secs(),
        "DNS resolver loop running"
    );

    // Per-service publication state. Empty after a (re)start, so the first pass
    // reconciles every target from scratch.
    let mut last_applied: HashMap<String, ServiceDnsState> = HashMap::new();
    let grid = RefreshGrid::new(period);
    let mut next_wake = tokio::time::Instant::now();
    let mut last_pass: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(next_wake) => {}
            _ = runtime.dns_resolver_targets.changed() => {}
        }
        // Coalesce bursts of target changes (for example the initial cache load).
        if let Some(last) = last_pass {
            tokio::time::sleep_until(last + MIN_REFRESH).await;
        }
        let now = tokio::time::Instant::now();
        last_pass = Some(now);

        // An empty table before the Service cache is ready must not GC slices.
        if !service_cache_ready(&runtime) {
            next_wake = grid.next_after(now);
            continue;
        }
        next_wake = match reconcile_with_runtime(&client, &watch_namespaces, &config, grid, &mut last_applied, &runtime)
            .await
        {
            Ok(next) => next,
            Err(e) => {
                tracing::warn!(
                    component = "dns_resolver_service",
                    error = %format!("{e:#}"),
                    "DNS resolver pass failed"
                );
                grid.next_after(now)
            }
        };
    }
}

fn service_cache_ready(runtime: &ControllerRuntime) -> bool {
    runtime
        .processor_registry
        .get("Service")
        .is_some_and(|processor| processor.is_ready())
}

/// Shared refresh schedule anchored at loop start. Every due instant sits on
/// this grid, so targets added at different times converge on the same passes
/// instead of each starting its own phase.
#[derive(Clone, Copy)]
struct RefreshGrid {
    epoch: tokio::time::Instant,
    period: Duration,
}

impl RefreshGrid {
    fn new(period: Duration) -> Self {
        Self {
            epoch: tokio::time::Instant::now(),
            period,
        }
    }

    /// First grid instant strictly after `now`; `now` itself for a zero period.
    fn next_after(&self, now: tokio::time::Instant) -> tokio::time::Instant {
        if self.period.is_zero() {
            return now;
        }
        let elapsed = now.saturating_duration_since(self.epoch);
        let ticks = elapsed.as_nanos() / self.period.as_nanos() + 1;
        let offset = u32::try_from(ticks)
            .ok()
            .and_then(|ticks| self.period.checked_mul(ticks))
            .unwrap_or(self.period);
        self.epoch + offset
    }
}

/// Next refresh for a Service handled now. DNS TTLs are not yet surfaced by
/// `edgion_common::dns::resolve`, so every Service refreshes on the grid.
fn next_refresh(now: tokio::time::Instant, grid: RefreshGrid) -> tokio::time::Instant {
    grid.next_after(now)
}

/// A due Service whose DNS query may run concurrently with others.
struct PendingResolution {
    spec: ResolverSpec,
    external_name: String,
    family: DnsLookupFamily,
    ports: Vec<ServicePort>,
    policy_cleanup_ok: bool,
}

/// One reconcile pass over the due targets. Returns the next wake instant.
///
/// Three phases: (1) prepare every due target serially (policy withdrawal and
/// resolver selection, which may write); (2) run the DNS queries concurrently
/// with bounded fan-out and a per-query deadline; (3) publish each result
/// serially as it arrives, then GC every resolver-owned slice whose Service is no longer a
/// target (mode switched away, type changed off ExternalName, disabled, or
/// Service deleted). The ownerReference alone only covers actual Service
/// deletion, not an in-place type change.
///
/// The set of resolver-owned slices is listed ONCE up front and reused for both
/// the GC pass and the `observed` existence check that gates the churn-skip
/// (`apply_resolved_slices`): without the existence check, a slice deleted
/// out-of-band while the resolved IPs are unchanged would never be recreated
/// (the input fingerprint would still match).
async fn reconcile_with_runtime(
    client: &Client,
    watch_namespaces: &[String],
    config: &DnsResolverConfig,
    grid: RefreshGrid,
    last_applied: &mut HashMap<String, ServiceDnsState>,
    runtime: &ControllerRuntime,
) -> Result<tokio::time::Instant> {
    let owned = list_owned_slices(client, watch_namespaces).await?;
    let observed: HashMap<(&str, &str), &EndpointSlice> = owned
        .iter()
        .filter_map(|slice| {
            Some((
                (slice.metadata.namespace.as_deref()?, slice.metadata.name.as_deref()?),
                slice,
            ))
        })
        .collect();

    let now = tokio::time::Instant::now();
    // Targets due within this window join the pass instead of waking it again.
    let horizon = now + MIN_REFRESH;
    let targets = runtime.dns_resolver_targets.snapshot();
    // A runtime reset clears processors before the table, so an empty snapshot
    // taken during leadership loss fails this check and never reaches GC.
    if !service_cache_ready(runtime) {
        return Ok(grid.next_after(now));
    }
    let desired: HashSet<String> = targets.iter().map(|(key, _)| key.clone()).collect();
    let default_candidate = controller_default_candidate(config);

    let mut pending = Vec::new();
    for (key, target) in targets {
        let state = last_applied.entry(key).or_default();
        let unchanged = state.target_revision == Some(target.revision);
        if unchanged && state.next_due.is_some_and(|due| due > horizon) {
            continue;
        }
        state.target_revision = Some(target.revision);
        // Measured from the horizon: a target pulled forward into this pass must
        // not come due again at the grid tick the horizon already covered.
        state.next_due = Some(next_refresh(horizon, grid));
        let service = target.service;
        if let Some(resolution) = prepare_resolution(
            client,
            &service,
            &default_candidate,
            config.lookup_family,
            last_applied,
            &observed,
            runtime,
        )
        .await
        {
            pending.push((service, resolution));
        }
    }

    // Publish each answer as it arrives so a slow resolver delays only itself.
    let mut resolved = futures::stream::iter(pending.into_iter().map(|(service, resolution)| async move {
        let result = query(&resolution).await;
        (service, resolution, result)
    }))
    .buffer_unordered(MAX_CONCURRENT_RESOLUTIONS);
    while let Some((service, resolution, result)) = resolved.next().await {
        finish_resolution(client, &service, resolution, result, last_applied, &observed).await;
    }

    if service_cache_ready(runtime) {
        gc_orphaned_slices(client, &owned, &desired, last_applied).await;
    }
    Ok(last_applied
        .values()
        .filter_map(|state| state.next_due)
        .min()
        .unwrap_or_else(|| next_refresh(now, grid)))
}

async fn query(resolution: &PendingResolution) -> Result<Vec<IpAddr>, String> {
    let lookup = resolve(
        &resolution.spec,
        &resolution.external_name,
        DnsQueryPolicy::Family(resolution.family),
    );
    match tokio::time::timeout(RESOLVE_DEADLINE, lookup).await {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => Err(format!("deadline of {}s exceeded", RESOLVE_DEADLINE.as_secs())),
    }
}

/// List resolver-owned slices from the apiserver watch cache (`resourceVersion=0`).
/// The Controller's EndpointSlice cache is not used: the operator-configured
/// global label selector applies to it, and resolver-owned slices do not carry
/// operator labels.
async fn list_owned_slices(client: &Client, watch_namespaces: &[String]) -> Result<Vec<EndpointSlice>> {
    let params = ListParams::default().labels(&managed_by_selector()).match_any();
    if watch_namespaces.is_empty() {
        return Ok(Api::<EndpointSlice>::all(client.clone()).list(&params).await?.items);
    }

    let mut items = Vec::new();
    for namespace in watch_namespaces {
        items.extend(
            Api::<EndpointSlice>::namespaced(client.clone(), namespace)
                .list(&params)
                .await?
                .items,
        );
    }
    Ok(items)
}

/// Serial preparation: withdraw policy-forbidden families, then select the
/// resolver. `None` when there is nothing to query.
///
/// On resolution failure only families allowed by the current policy retain
/// last-good slices. Policy withdrawals run before resolver or DNS I/O.
async fn prepare_resolution(
    client: &Client,
    svc: &Service,
    default_candidate: &Option<ResolverCandidate>,
    default_family: DnsLookupFamily,
    last_applied: &mut HashMap<String, ServiceDnsState>,
    observed: &HashMap<(&str, &str), &EndpointSlice>,
    runtime: &ControllerRuntime,
) -> Option<PendingResolution> {
    let name = svc.metadata.name.as_deref()?;
    let namespace = svc.metadata.namespace.as_deref().unwrap_or("default");
    let external_name = svc
        .spec
        .as_ref()
        .filter(|spec| spec.type_.as_deref() == Some("ExternalName"))
        .and_then(|spec| spec.external_name.clone())?;
    let service_key = format!("{namespace}/{name}");

    let family = service_lookup_family(svc, default_family);
    let policy_cleanup_ok = delete_unwanted_slices(client, namespace, name, observed, |v6| {
        family.is_ok_and(|family| family.allows(family_probe(v6)))
    })
    .await;
    let family = match family {
        Ok(family) => family,
        Err(error) => {
            last_applied.entry(service_key.clone()).or_default().applied_fingerprint = None;
            tracing::warn!(component = "dns_resolver_service", service = %service_key, error,
                "Invalid DNS family policy; withdrawing resolver-owned endpoints without querying");
            return None;
        }
    };

    let ports: Vec<ServicePort> = svc.spec.as_ref().and_then(|s| s.ports.clone()).unwrap_or_default();
    if ports.is_empty() {
        // No declared ports → nothing routable to publish; the gateway bridges
        // via self-resolution. Avoid a useless port-less slice.
        tracing::debug!(
            component = "dns_resolver_service",
            service = %service_key,
            "ExternalName Service declares no ports; skipping (gateway will self-resolve)"
        );
        return None;
    }

    if svc.metadata.uid.is_none() {
        tracing::debug!(
            component = "dns_resolver_service",
            service = %service_key,
            "Service missing uid; skipping (cannot set ownerReference)"
        );
        return None;
    }

    let state = last_applied.entry(service_key).or_default();
    let candidate = service_resolver_candidate(svc, runtime);
    let spec = effective_spec(runtime, candidate, default_candidate, state);
    Some(PendingResolution {
        spec,
        external_name,
        family,
        ports,
        policy_cleanup_ok,
    })
}

/// Serial publication of one query result.
async fn finish_resolution(
    client: &Client,
    svc: &Service,
    resolution: PendingResolution,
    result: Result<Vec<IpAddr>, String>,
    last_applied: &mut HashMap<String, ServiceDnsState>,
    observed: &HashMap<(&str, &str), &EndpointSlice>,
) {
    let mut ips = match result {
        Ok(ips) => ips,
        Err(error) => {
            tracing::warn!(
                component = "dns_resolver_service",
                namespace = svc.metadata.namespace.as_deref().unwrap_or("default"),
                service = svc.metadata.name.as_deref().unwrap_or(""),
                external_name = %resolution.external_name,
                error = %error,
                "DNS resolution failed; keeping last-good EndpointSlice"
            );
            return;
        }
    };
    apply_resolved_slices(
        client,
        svc,
        &resolution.ports,
        &mut ips,
        resolution.family,
        resolution.policy_cleanup_ok,
        last_applied,
        observed,
    )
    .await;
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

fn family_probe(v6: bool) -> IpAddr {
    if v6 {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    }
}

/// Publish one resolved generation, invalidating the prior fingerprint before writes.
#[allow(clippy::too_many_arguments)]
async fn apply_resolved_slices(
    client: &Client,
    svc: &Service,
    ports: &[ServicePort],
    ips: &mut Vec<IpAddr>,
    family: DnsLookupFamily,
    policy_cleanup_ok: bool,
    last_applied: &mut HashMap<String, ServiceDnsState>,
    observed: &HashMap<(&str, &str), &EndpointSlice>,
) {
    let Some(name) = svc.metadata.name.as_deref() else {
        return;
    };
    let namespace = svc.metadata.namespace.as_deref().unwrap_or("default");
    let Some(owner) = service_owner_ref(svc) else {
        return;
    };
    let service_key = format!("{namespace}/{name}");
    // Deterministic order + de-dup: an unchanged resolution then yields an
    // identical fingerprint (no spurious re-apply) and a stable published slice.
    ips.sort();
    ips.dedup();
    if ips.is_empty() {
        return;
    }

    // Unchanged content may skip only when all desired families still belong
    // to the current Service UID; otherwise normal forced SSA repairs ownership.
    let fingerprint = slice_fingerprint(ips, ports, family);
    if policy_cleanup_ok && can_skip_owned_slices(observed, last_applied, &service_key, &fingerprint, svc, ips) {
        return;
    }

    let resolved_at = now_rfc3339();
    let slices = build_endpoint_slices(name, namespace, owner, ports, ips, &resolved_at);
    if slices.is_empty() {
        return;
    }

    // A partial write (or cancellation) invalidates the old success claim. If DNS
    // returns to that old answer later, it must repair the intermediate content.
    if let Some(state) = last_applied.get_mut(&service_key) {
        state.applied_fingerprint = None;
    }
    let api: Api<EndpointSlice> = Api::namespaced(client.clone(), namespace);
    let pp = PatchParams::apply(FIELD_MANAGER).force();
    let mut all_ok = true;
    for slice in &slices {
        let slice_name = slice.metadata.name.clone().unwrap_or_default();
        if let Err(e) = api.patch(&slice_name, &pp, &Patch::Apply(slice)).await {
            tracing::warn!(
                component = "dns_resolver_service",
                service = %service_key,
                slice = %slice_name,
                error = %e,
                "Failed to apply resolver EndpointSlice"
            );
            all_ok = false;
        }
    }

    if all_ok {
        all_ok = delete_unwanted_slices(client, namespace, name, observed, |v6| {
            ips.iter().any(|ip| ip.is_ipv6() == v6)
        })
        .await;
    }
    // Failed publication or withdrawal must retry, even for an unchanged answer.
    if all_ok && policy_cleanup_ok {
        last_applied.entry(service_key.clone()).or_default().applied_fingerprint = Some(fingerprint);
        tracing::debug!(
            component = "dns_resolver_service",
            service = %service_key,
            addresses = ips.len(),
            slices = slices.len(),
            "Published resolver EndpointSlice(s)"
        );
    }
}

/// Every required family must belong to this current Service generation.
fn can_skip_owned_slices(
    observed: &HashMap<(&str, &str), &EndpointSlice>,
    last_applied: &HashMap<String, ServiceDnsState>,
    service_key: &str,
    fingerprint: &str,
    service: &Service,
    ips: &[IpAddr],
) -> bool {
    if last_applied
        .get(service_key)
        .and_then(|state| state.applied_fingerprint.as_deref())
        != Some(fingerprint)
    {
        return false;
    }
    let (Some(name), Some(uid)) = (service.metadata.name.as_deref(), service.metadata.uid.as_deref()) else {
        return false;
    };
    let namespace = service.metadata.namespace.as_deref().unwrap_or("default");
    [false, true].into_iter().all(|v6| {
        let expected = slice_name(name, v6);
        if !ips.iter().any(|ip| ip.is_ipv6() == v6) {
            return !observed.contains_key(&(namespace, expected.as_str()));
        }
        observed.get(&(namespace, expected.as_str())).is_some_and(|slice| {
            slice.address_type == if v6 { "IPv6" } else { "IPv4" }
                && proven_owned_slice(slice, namespace, name, v6)
                && slice.metadata.namespace.as_deref() == Some(namespace)
                && slice.metadata.name.as_deref() == Some(expected.as_str())
                && slice
                    .metadata
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get(SERVICE_NAME))
                    .map(String::as_str)
                    == Some(name)
                && slice.metadata.owner_references.as_ref().is_some_and(|owners| {
                    owners.iter().any(|owner| {
                        owner.api_version == "v1" && owner.kind == "Service" && owner.name == name && owner.uid == uid
                    })
                })
        })
    })
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

/// Publication identity using the caller's sorted/deduplicated IPs and the
/// builder's complete EndpointPort projection. JSON preserves optional-field
/// distinctions; sorting the encoded ports ignores Service port ordering.
fn slice_fingerprint(ips: &[IpAddr], ports: &[ServicePort], family: DnsLookupFamily) -> String {
    let ip_part = ips.iter().map(|ip| ip.to_string()).collect::<Vec<_>>().join(",");
    let mut port_parts: Vec<String> = endpoint_ports(ports)
        .into_iter()
        .map(|port| serde_json::json!(port).to_string())
        .collect();
    port_parts.sort_unstable();
    format!("{family:?}|{ip_part}|[{}]", port_parts.join(","))
}

/// Extract the `namespace/service-name` key of a resolver-owned slice from its
/// namespace + `kubernetes.io/service-name` label. `None` for a slice missing
/// either (our builder always sets both).
fn owned_slice_service_key(slice: &EndpointSlice) -> Option<String> {
    let ns = slice.metadata.namespace.as_deref()?;
    let service_name = slice.metadata.labels.as_ref()?.get(SERVICE_NAME)?;
    Some(format!("{ns}/{service_name}"))
}

/// Compute the control-plane effective resolver for a Service: Service
/// annotation → Controller-YAML default (control-plane precedence — no Gateway /
/// EGC layers). httpdns refs resolve against the processed LinkSys cache, which
/// carries the controller-resolved TLS material.
fn effective_spec(
    runtime: &ControllerRuntime,
    service_candidate: Option<ResolverCandidate>,
    default_candidate: &Option<ResolverCandidate>,
    state: &mut ServiceDnsState,
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

/// Resolve the Controller-wide default through the same precedence and
/// LinkSys lookup used by the ExternalName reconciliation service.
pub(crate) fn controller_default_resolver(runtime: &ControllerRuntime) -> ResolverSpec {
    let config = dns_resolver_config();
    let default_candidate = controller_default_candidate(&config);
    effective_spec(runtime, None, &default_candidate, &mut ServiceDnsState::default())
}

/// Look up a processed httpdns LinkSys owner. Returns `None` when the object is
/// absent or is not an httpdns LinkSys (a dangling ref then fails safe to the
/// system resolver in `resolve_effective_spec`).
fn processed_httpdns_owner(runtime: &ControllerRuntime, key: &LinkSysKey) -> Option<Arc<LinkSys>> {
    let cached = runtime
        .processor_registry
        .get("LinkSys")?
        .get_cached_any(&format!("{}/{}", key.namespace, key.name))?;
    let owner = cached.downcast::<LinkSys>().ok()?;
    owner.spec.config.as_http_dns().is_some().then_some(owner)
}

/// Build the control-plane default resolver candidate from Controller config.
/// `link_sys_ref` (httpdns) wins over `servers` (inline standard DNS); the two
/// are validated mutually exclusive at config load.
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

/// Prove ownership independently of the current Service UID: a recreated Service
/// must also be able to retire a sibling left by its previous incarnation.
fn proven_owned_slice(slice: &EndpointSlice, namespace: &str, service: &str, v6: bool) -> bool {
    let expected = slice_name(service, v6);
    slice.metadata.namespace.as_deref() == Some(namespace)
        && slice.metadata.name.as_deref() == Some(expected.as_str())
        && slice.metadata.labels.as_ref().is_some_and(|labels| {
            labels.get(SERVICE_NAME).map(String::as_str) == Some(service)
                && labels.get(ENDPOINTSLICE_MANAGED_BY).map(String::as_str) == Some(MANAGED_BY_VALUE)
                && labels.get(SYNTHETIC).map(String::as_str) == Some("true")
        })
        && slice.metadata.owner_references.as_ref().is_some_and(|owners| {
            owners.iter().any(|owner| {
                owner.api_version == "v1" && owner.kind == "Service" && owner.name == service && !owner.uid.is_empty()
            })
        })
}

/// Delete only the exact observed object. A concurrent replacement must survive.
async fn delete_owned_slice(client: &Client, slice: &EndpointSlice, namespace: &str, service: &str, v6: bool) -> bool {
    let name = slice_name(service, v6);
    let metadata = &slice.metadata;
    let proof = proven_owned_slice(slice, namespace, service, v6);
    let identity = metadata
        .uid
        .as_ref()
        .filter(|value| !value.is_empty())
        .zip(metadata.resource_version.as_ref().filter(|value| !value.is_empty()));
    let Some((uid, resource_version)) = identity.filter(|_| proof) else {
        tracing::warn!(component = "dns_resolver_service", namespace, service, slice = %name,
            "Cannot prove resolver EndpointSlice ownership and deletion identity; preserving object");
        return false;
    };
    let params = DeleteParams {
        preconditions: Some(Preconditions {
            uid: Some(uid.clone()),
            resource_version: Some(resource_version.clone()),
        }),
        ..Default::default()
    };
    match Api::<EndpointSlice>::namespaced(client.clone(), namespace)
        .delete(&name, &params)
        .await
    {
        Ok(_) => true,
        Err(kube::Error::Api(error)) if error.code == 404 => true,
        Err(error) => {
            tracing::warn!(component = "dns_resolver_service", namespace, service, slice = %name, error = %error,
                "Failed to delete resolver EndpointSlice; retrying on next reconciliation");
            false
        }
    }
}

async fn delete_unwanted_slices(
    client: &Client,
    namespace: &str,
    service: &str,
    observed: &HashMap<(&str, &str), &EndpointSlice>,
    keep: impl Fn(bool) -> bool,
) -> bool {
    let mut complete = true;
    for v6 in [false, true] {
        if keep(v6) {
            continue;
        }
        let name = slice_name(service, v6);
        if let Some(slice) = observed.get(&(namespace, name.as_str())) {
            complete &= delete_owned_slice(client, slice, namespace, service, v6).await;
        }
    }
    complete
}

/// GC every resolver-owned EndpointSlice (from the pre-listed `owned` set) whose
/// backing Service is not in `desired` (mode switched away, type changed off
/// ExternalName, disabled, or Service deleted). Deletes are best-effort (logged,
/// not propagated). Also prunes `last_applied` down to the desired set so it
/// cannot grow without bound.
async fn gc_orphaned_slices(
    client: &Client,
    owned: &[EndpointSlice],
    desired: &HashSet<String>,
    last_applied: &mut HashMap<String, ServiceDnsState>,
) {
    for slice in owned {
        let Some(service_key) = owned_slice_service_key(slice) else {
            continue;
        };
        if desired.contains(&service_key) {
            continue;
        }
        let (Some(ns), Some(name)) = (slice.metadata.namespace.as_deref(), slice.metadata.name.as_deref()) else {
            continue;
        };
        let Some(service) = slice
            .metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get(SERVICE_NAME))
        else {
            continue;
        };
        let v6 = name == slice_name(service, true);
        delete_owned_slice(client, slice, ns, service, v6).await;
    }

    last_applied.retain(|k, _| desired.contains(k));
}

/// Build an OwnerReference to the Service so K8s garbage-collects the slice when
/// the Service is deleted. Returns `None` when the Service has no uid.
fn service_owner_ref(svc: &Service) -> Option<OwnerReference> {
    let ObjectMeta { name, uid, .. } = &svc.metadata;
    Some(OwnerReference {
        api_version: "v1".to_string(),
        kind: "Service".to_string(),
        name: name.clone()?,
        uid: uid.clone()?,
        controller: Some(false),
        block_owner_deletion: Some(false),
    })
}

/// Parse a `refresh_interval` duration string (e.g. `"15s"`, `"500ms"`, `"1m"`).
/// Unparseable values fall back to [`DEFAULT_REFRESH`]; anything below
/// [`MIN_REFRESH`] (including zero) is clamped up so a misconfiguration cannot
/// turn the loop into an apiserver-hammering storm.
fn parse_refresh_interval(value: &str) -> Duration {
    parse_duration_str(value).unwrap_or(DEFAULT_REFRESH).max(MIN_REFRESH)
}

/// Parse a GEP-2257 duration string (`"15s"`, `"500ms"`, `"2m"`, `"1h30m"`).
/// Delegates to the canonical [`edgion_resources::duration::parse_duration`], so
/// every duration field in the workspace shares one grammar.
/// Shared with `DnsResolverConfig::validate` for load-time rejection.
pub(crate) fn parse_duration_str(value: &str) -> Option<Duration> {
    edgion_resources::duration::parse_duration(value).ok()
}

#[cfg(test)]
#[path = "../../../tests/unit/services/dns_resolver/service_test.rs"]
mod tests;
