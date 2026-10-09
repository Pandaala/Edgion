//! Control-plane DNS resolver service.
//!
//! A leader-only background loop (started alongside the ACME service, K8s
//! conf_center mode only) that, on an interval, resolves `mode=controller`
//! ExternalName Services via their effective resolver and writes a **real**
//! `EndpointSlice` to the apiserver. That slice flows to the gateway through the
//! existing reflector → conf_sync → EndpointSlice-LB path, giving the
//! "control-plane resolves centrally, multi-address LB" behavior (Envoy
//! STRICT_DNS-like).
//!
//! In FileSystem/Etcd conf_center modes this service is never started; there,
//! `mode=controller` Services degrade to the gateway self-resolution bridge.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use k8s_openapi::api::core::v1::{Service, ServicePort};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, DeleteParams, ListParams, ObjectMeta, Patch, PatchParams};
use kube::Client;
use parking_lot::Mutex;

use crate::conf_mgr::sync_runtime::resource_processor::handlers::dns_resolver_guard::dns_query_allowed;
use crate::conf_mgr::ControllerRuntime;
use edgion_common::dns::resolver_select::{
    parse_dns_server_list, resolve_effective_spec, LinkSysKey, ResolverCandidate,
};
use edgion_common::dns::{resolve, HttpDnsCandidate, PreparedHttpDns, ResolverSpec};
use edgion_common::time::now_rfc3339;
use edgion_resources::constants::annotations::edgion::{DNS_RESOLUTION_MODE, DNS_RESOLVER as DNS_RESOLVER_ANNOTATION};
use edgion_resources::constants::labels::k8s::SERVICE_NAME;
use edgion_resources::resources::link_sys::LinkSys;
use edgion_resources::resources::refs::dns_fields::DnsResolverOwner;

use crate::config::{dns_resolver_config, DnsResolverConfig};

use super::slice_builder::{
    build_endpoint_slices, decide_action, managed_by_selector, slice_name, SliceAction, FIELD_MANAGER,
};

/// Fallback tick interval when the configured `refresh_interval` is unparseable.
const DEFAULT_REFRESH: Duration = Duration::from_secs(15);

/// Lower bound on the tick interval. A tiny interval would hammer the apiserver
/// (list-all Services + resolve + patch storm), so anything below this is clamped
/// up. Also enforced at config load (`DnsResolverConfig::validate`).
const MIN_REFRESH: Duration = Duration::from_secs(1);

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

/// The loop body. Ticks on `refresh_interval` and reconciles every tick.
async fn run(client: Client, watch_namespaces: Vec<String>, runtime: Arc<ControllerRuntime>) {
    let config = dns_resolver_config();
    let period = parse_refresh_interval(&config.refresh_interval);
    tracing::info!(
        component = "dns_resolver_service",
        enabled = config.enabled,
        interval_secs = period.as_secs(),
        "DNS resolver loop running"
    );

    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Per-service fingerprint (sorted IPs + ports) of the last slice content we
    // applied. Lets a tick skip the SSA write when nothing changed, avoiding
    // data-plane churn every interval. Empty after a (re)start → the first tick
    // reconciles everything from scratch.
    let mut last_applied: HashMap<String, ServiceDnsState> = HashMap::new();

    loop {
        ticker.tick().await;
        // Re-read config each tick so a hot reload of `enabled` / default resolver
        // takes effect without restarting the service.
        let config = dns_resolver_config();
        if let Err(e) = reconcile_with_runtime(&client, &watch_namespaces, &config, &mut last_applied, &runtime).await {
            tracing::warn!(
                component = "dns_resolver_service",
                error = %format!("{e:#}"),
                "DNS resolver tick failed"
            );
        }
    }
}

/// One reconcile pass over all Services.
///
/// Two phases: (1) resolve+apply a slice for every controller-mode ExternalName
/// Service, recording its key in `desired`; (2) GC every resolver-owned slice
/// whose Service is not in `desired` (mode switched away, type changed off
/// ExternalName, disabled, or Service deleted). Phase 2 is what catches a Service
/// edited off ExternalName — the ownerReference alone only covers actual Service
/// deletion, not an in-place type change.
///
/// The set of resolver-owned slices is listed ONCE up front and reused for both
/// the GC pass and the `observed` existence check that gates the churn-skip
/// (`resolve_and_apply`): without the existence check, a slice deleted
/// out-of-band while the resolved IPs are unchanged would never be recreated
/// (the input fingerprint would still match). Listing first heals it in-tick.
async fn reconcile_with_runtime(
    client: &Client,
    watch_namespaces: &[String],
    config: &DnsResolverConfig,
    last_applied: &mut HashMap<String, ServiceDnsState>,
    runtime: &ControllerRuntime,
) -> Result<()> {
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
    let services = list_services(client, watch_namespaces).await?;

    let default_candidate = controller_default_candidate(config);
    let mut desired: HashSet<String> = HashSet::new();

    for svc in services {
        let namespace = svc.metadata.namespace.clone().unwrap_or_else(|| "default".to_string());
        let Some(name) = svc.metadata.name.clone() else {
            continue;
        };

        let external_name = svc
            .spec
            .as_ref()
            .filter(|s| s.type_.as_deref() == Some("ExternalName"))
            .and_then(|s| s.external_name.clone());
        let is_external_name = external_name.is_some();

        let mode_value = svc
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(DNS_RESOLUTION_MODE))
            .map(String::as_str);

        // Only `Resolve` produces/keeps a slice; `GarbageCollect`/`Skip` are left
        // to the owned-slice GC pass below (which also handles deleted Services).
        if decide_action(config.enabled, is_external_name, mode_value) == SliceAction::Resolve {
            desired.insert(format!("{namespace}/{name}"));
            // Safe: `is_external_name` gated the Resolve arm.
            let external_name = external_name.unwrap_or_default();
            resolve_and_apply(
                client,
                &svc,
                &namespace,
                &name,
                &external_name,
                &default_candidate,
                last_applied,
                &observed,
                runtime,
            )
            .await;
        }
    }

    gc_orphaned_slices(client, &owned, &desired, last_applied).await;
    Ok(())
}

async fn list_owned_slices(client: &Client, watch_namespaces: &[String]) -> Result<Vec<EndpointSlice>> {
    let params = ListParams::default().labels(&managed_by_selector());
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

async fn list_services(client: &Client, watch_namespaces: &[String]) -> Result<Vec<Service>> {
    let params = ListParams::default();
    if watch_namespaces.is_empty() {
        return Ok(Api::<Service>::all(client.clone()).list(&params).await?.items);
    }

    let mut items = Vec::new();
    for namespace in watch_namespaces {
        items.extend(
            Api::<Service>::namespaced(client.clone(), namespace)
                .list(&params)
                .await?
                .items,
        );
    }
    Ok(items)
}

/// Resolve one ExternalName Service and apply its EndpointSlice(s).
///
/// On resolution failure the last-good slice is intentionally left in place (we
/// do NOT delete or empty it) — thrashing to no endpoints would flap the backend
/// to 503 on a transient DNS blip.
#[allow(clippy::too_many_arguments)]
async fn resolve_and_apply(
    client: &Client,
    svc: &Service,
    namespace: &str,
    name: &str,
    external_name: &str,
    default_candidate: &Option<ResolverCandidate>,
    last_applied: &mut HashMap<String, ServiceDnsState>,
    observed: &HashMap<(&str, &str), &EndpointSlice>,
    runtime: &ControllerRuntime,
) {
    let service_key = format!("{namespace}/{name}");

    let ports: Vec<ServicePort> = svc.spec.as_ref().and_then(|s| s.ports.clone()).unwrap_or_default();
    if ports.is_empty() {
        // No declared ports → nothing routable to publish; the gateway bridges
        // via self-resolution. Avoid a useless port-less slice.
        tracing::debug!(
            component = "dns_resolver_service",
            service = %service_key,
            "ExternalName Service declares no ports; skipping (gateway will self-resolve)"
        );
        return;
    }

    if svc.metadata.uid.is_none() {
        tracing::debug!(
            component = "dns_resolver_service",
            service = %service_key,
            "Service missing uid; skipping (cannot set ownerReference)"
        );
        return;
    }

    let state = last_applied.entry(service_key.clone()).or_default();
    let candidate = service_resolver_candidate(svc, runtime);
    let spec = effective_spec(client, candidate, default_candidate, state).await;

    let mut ips = match resolve(&spec, external_name).await {
        Ok(ips) => ips,
        Err(e) => {
            tracing::warn!(
                component = "dns_resolver_service",
                service = %service_key,
                external_name = %external_name,
                error = %e,
                "DNS resolution failed; keeping last-good EndpointSlice"
            );
            return;
        }
    };
    apply_resolved_slices(client, svc, &ports, &mut ips, last_applied, observed).await;
}

/// Publish one resolved generation, retaining the prior fingerprint on partial failure.
async fn apply_resolved_slices(
    client: &Client,
    svc: &Service,
    ports: &[ServicePort],
    ips: &mut Vec<IpAddr>,
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
    let fingerprint = slice_fingerprint(ips, ports);
    if can_skip_owned_slices(observed, last_applied, &service_key, &fingerprint, svc, ips) {
        return;
    }

    let resolved_at = now_rfc3339();
    let slices = build_endpoint_slices(name, namespace, owner, ports, ips, &resolved_at);
    if slices.is_empty() {
        return;
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

    // Only record the fingerprint on a fully successful apply, so a partial
    // failure is retried (re-applied) on the next tick.
    if all_ok {
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
    [false, true]
        .into_iter()
        .filter(|v6| ips.iter().any(|ip| ip.is_ipv6() == *v6))
        .all(|v6| {
            let expected = slice_name(name, v6);
            observed.get(&(namespace, expected.as_str())).is_some_and(|slice| {
                slice.address_type == if v6 { "IPv6" } else { "IPv4" }
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
                            owner.api_version == "v1"
                                && owner.kind == "Service"
                                && owner.name == name
                                && owner.uid == uid
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
    (!servers.is_empty()).then_some(ResolverCandidate::Servers(servers))
}

/// Content fingerprint (sorted IPs + sorted Service port numbers) used to detect
/// whether a re-apply is actually needed this tick.
///
/// NOTE: intentionally covers only IPs + port NUMBERS, not port name / protocol /
/// appProtocol. The gateway matches endpoints on service-name + port number, so a
/// name/protocol-only change does not affect routing; that metadata on the
/// published slice may lag until the IP set or a port number changes.
fn slice_fingerprint(ips: &[IpAddr], ports: &[ServicePort]) -> String {
    let ip_part = ips.iter().map(|ip| ip.to_string()).collect::<Vec<_>>().join(",");
    let mut port_nums: Vec<i32> = ports.iter().map(|p| p.port).collect();
    port_nums.sort_unstable();
    let port_part = port_nums.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
    format!("{ip_part}|{port_part}")
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
/// EGC layers). httpdns refs are resolved against the apiserver LinkSys objects.
///
/// NOTE: resolved secret material (mTLS client certs on an httpdns
/// `connection.tls`) is NOT populated here — this reads the raw LinkSys CR, whose
/// resolved-secret fields are `#[schemars(skip)]` and filled only on the
/// conf_sync path. A plain-HTTP(S) httpdns endpoint resolves fine; an mTLS one
/// degrades to the config's `fallback` (see `edgion_common::dns::resolve`).
async fn effective_spec(
    client: &Client,
    service_candidate: Option<ResolverCandidate>,
    default_candidate: &Option<ResolverCandidate>,
    state: &mut ServiceDnsState,
) -> ResolverSpec {
    let layers = [service_candidate, default_candidate.clone()];
    let prepared = match layers.iter().flatten().next() {
        Some(ResolverCandidate::HttpDnsRef(key)) => match fetch_httpdns_owner(client, key).await {
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
pub(crate) async fn controller_default_resolver(client: &Client) -> ResolverSpec {
    let config = dns_resolver_config();
    let default_candidate = controller_default_candidate(&config);
    effective_spec(client, None, &default_candidate, &mut ServiceDnsState::default()).await
}

/// Kube-less fallback for persist-off acquisition on FS/etcd.
///
/// Reuses the Controller default candidate. An `HttpDnsRef` cannot be fetched
/// from the apiserver here, so `resolve_effective_spec` fails safe to the
/// system resolver (same dangling-ref behavior as a missing LinkSys).
pub(crate) fn controller_default_resolver_without_kube() -> ResolverSpec {
    let config = dns_resolver_config();
    let default_candidate = controller_default_candidate(&config);
    resolve_effective_spec(&[default_candidate], |_| None)
}

/// Fetch an actual httpdns LinkSys owner from the apiserver. Returns `None`
/// when the object is missing or is not an httpdns LinkSys (a dangling ref then
/// fails safe to the system resolver in `resolve_effective_spec`).
async fn fetch_httpdns_owner(client: &Client, key: &LinkSysKey) -> Option<Arc<LinkSys>> {
    let api: Api<LinkSys> = Api::namespaced(client.clone(), &key.namespace);
    match api.get(&key.name).await {
        Ok(ls) if ls.spec.config.as_http_dns().is_some() => Some(Arc::new(ls)),
        Ok(_) => None,
        Err(_) => None,
    }
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
        return Some(ResolverCandidate::Servers(config.servers.clone()));
    }
    None
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
        let (Some(ns), Some(slice_name)) = (slice.metadata.namespace.as_deref(), slice.metadata.name.as_deref()) else {
            continue;
        };
        let sapi: Api<EndpointSlice> = Api::namespaced(client.clone(), ns);
        match sapi.delete(slice_name, &DeleteParams::default()).await {
            Ok(_) => tracing::debug!(
                component = "dns_resolver_service",
                service = %service_key,
                slice = %slice_name,
                "Garbage-collected orphaned resolver EndpointSlice"
            ),
            Err(e) => tracing::warn!(
                component = "dns_resolver_service",
                service = %service_key,
                slice = %slice_name,
                error = %e,
                "Failed to delete orphaned resolver EndpointSlice"
            ),
        }
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
