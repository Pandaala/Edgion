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
use std::time::Duration;

use anyhow::Result;
use k8s_openapi::api::core::v1::{Service, ServicePort};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, DeleteParams, ListParams, ObjectMeta, Patch, PatchParams};
use kube::Client;
use parking_lot::Mutex;
use tokio::task::JoinHandle;

use edgion_common::dns::resolver_select::{
    candidate_from_annotations, resolve_effective_spec, LinkSysKey, ResolverCandidate,
};
use edgion_common::dns::{resolve, ResolverSpec};
use edgion_resources::constants::annotations::edgion::DNS_RESOLUTION_MODE;
use edgion_resources::constants::labels::k8s::SERVICE_NAME;
use edgion_resources::resources::link_sys::LinkSys;

use crate::config::{dns_resolver_config, DnsResolverConfig};

use super::slice_builder::{build_endpoint_slices, decide_action, managed_by_selector, SliceAction, FIELD_MANAGER};

/// Fallback tick interval when the configured `refresh_interval` is unparseable.
const DEFAULT_REFRESH: Duration = Duration::from_secs(15);

/// Lower bound on the tick interval. A tiny interval would hammer the apiserver
/// (list-all Services + resolve + patch storm), so anything below this is clamped
/// up. Also enforced at config load (`DnsResolverConfig::validate`).
const MIN_REFRESH: Duration = Duration::from_secs(1);

/// Global singleton handle. Holds the loop's `JoinHandle` so `stop` can abort it
/// promptly on leadership loss / shutdown (mirrors the leader-only ACME service).
static DNS_RESOLVER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// Start the control-plane DNS resolver loop. Idempotent: a warning + no-op if
/// already running (call [`stop_dns_resolver_service`] first on re-election).
pub fn start_dns_resolver_service(client: Client, watch_namespaces: Vec<String>) {
    let mut guard = DNS_RESOLVER.lock();
    if guard.is_some() {
        tracing::warn!(
            component = "dns_resolver_service",
            "DNS resolver service already running, call stop_dns_resolver_service() first"
        );
        return;
    }
    let handle = tokio::spawn(async move { run(client, watch_namespaces).await });
    *guard = Some(handle);
    tracing::info!(component = "dns_resolver_service", "DNS resolver service started");
}

/// Stop the loop (leadership loss / shutdown). Aborts the background task.
pub fn stop_dns_resolver_service() {
    if let Some(handle) = DNS_RESOLVER.lock().take() {
        handle.abort();
        tracing::info!(component = "dns_resolver_service", "DNS resolver service stopped");
    }
}

/// The loop body. Ticks on `refresh_interval` and reconciles every tick.
async fn run(client: Client, watch_namespaces: Vec<String>) {
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
    let mut last_applied: HashMap<String, String> = HashMap::new();

    loop {
        ticker.tick().await;
        // Re-read config each tick so a hot reload of `enabled` / default resolver
        // takes effect without restarting the service.
        let config = dns_resolver_config();
        if let Err(e) = reconcile_once(&client, &watch_namespaces, &config, &mut last_applied).await {
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
async fn reconcile_once(
    client: &Client,
    watch_namespaces: &[String],
    config: &DnsResolverConfig,
    last_applied: &mut HashMap<String, String>,
) -> Result<()> {
    let owned = list_owned_slices(client, watch_namespaces).await?;
    let observed: HashSet<String> = owned.iter().filter_map(owned_slice_service_key).collect();
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
    last_applied: &mut HashMap<String, String>,
    observed: &HashSet<String>,
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

    let Some(owner) = service_owner_ref(svc) else {
        tracing::debug!(
            component = "dns_resolver_service",
            service = %service_key,
            "Service missing uid; skipping (cannot set ownerReference)"
        );
        return;
    };

    let annotations = svc.metadata.annotations.clone().unwrap_or_default();
    let spec = effective_spec(client, &annotations, default_candidate).await;

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
    // Deterministic order + de-dup: an unchanged resolution then yields an
    // identical fingerprint (no spurious re-apply) and a stable published slice.
    ips.sort();
    ips.dedup();
    if ips.is_empty() {
        return;
    }

    // Skip the write only when the slice is BOTH still present (`observed`) AND
    // its content fingerprint (sorted IPs + ports) is unchanged. Gating on the
    // fingerprint alone would wrongly skip forever if the slice were deleted
    // out-of-band while the resolved IPs stayed the same; the existence check
    // heals that in the same tick. Otherwise the SSA `force` write would bump the
    // resourceVersion every tick and re-fire the whole conf_sync → data-plane path.
    let fingerprint = slice_fingerprint(&ips, &ports);
    if can_skip_apply(observed, last_applied, &service_key, &fingerprint) {
        return;
    }

    let resolved_at = chrono::Utc::now().to_rfc3339();
    let slices = build_endpoint_slices(name, namespace, owner, &ports, &ips, &resolved_at);
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
        last_applied.insert(service_key.clone(), fingerprint);
        tracing::debug!(
            component = "dns_resolver_service",
            service = %service_key,
            addresses = ips.len(),
            slices = slices.len(),
            "Published resolver EndpointSlice(s)"
        );
    }
}

/// Whether a tick may skip re-applying a Service's slice.
///
/// Requires BOTH that the slice is still observed to exist AND that its content
/// fingerprint is unchanged. The existence guard is what makes an out-of-band
/// deletion self-heal: an unchanged fingerprint alone would skip the write
/// forever and leave the backend with zero endpoints.
fn can_skip_apply(
    observed: &HashSet<String>,
    last_applied: &HashMap<String, String>,
    service_key: &str,
    fingerprint: &str,
) -> bool {
    observed.contains(service_key) && last_applied.get(service_key).map(|f| f == fingerprint).unwrap_or(false)
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
    svc_annotations: &std::collections::BTreeMap<String, String>,
    default_candidate: &Option<ResolverCandidate>,
) -> ResolverSpec {
    let service_candidate = candidate_from_annotations(svc_annotations);

    // Prefetch the httpdns configs any layer references, keyed by (ns, name),
    // so `resolve_effective_spec` can look them up synchronously.
    let mut cache: HashMap<
        (String, String),
        Option<Box<edgion_resources::resources::link_sys::HttpDnsResolverConfig>>,
    > = HashMap::new();
    for candidate in [&service_candidate, default_candidate].into_iter().flatten() {
        if let ResolverCandidate::HttpDnsRef(key) = candidate {
            let map_key = (key.namespace.clone(), key.name.clone());
            if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry(map_key) {
                let cfg = fetch_httpdns_config(client, key).await.map(Box::new);
                slot.insert(cfg);
            }
        }
    }

    resolve_effective_spec(
        &[service_candidate.clone(), default_candidate.clone()],
        |key: &LinkSysKey| {
            cache
                .get(&(key.namespace.clone(), key.name.clone()))
                .cloned()
                .flatten()
                .map(|boxed| *boxed)
        },
    )
}

/// Fetch an httpdns LinkSys's resolver config from the apiserver. Returns `None`
/// when the object is missing or is not an httpdns LinkSys (a dangling ref then
/// fails safe to the system resolver in `resolve_effective_spec`).
async fn fetch_httpdns_config(
    client: &Client,
    key: &LinkSysKey,
) -> Option<edgion_resources::resources::link_sys::HttpDnsResolverConfig> {
    let api: Api<LinkSys> = Api::namespaced(client.clone(), &key.namespace);
    match api.get(&key.name).await {
        Ok(ls) => ls.spec.config.as_http_dns().cloned(),
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
    last_applied: &mut HashMap<String, String>,
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

/// Parse a duration string (`"15s"`, `"500ms"`, `"2m"`, `"1h"`, bare = seconds).
/// Shared with `DnsResolverConfig::validate` for load-time rejection.
pub(crate) fn parse_duration_str(value: &str) -> Option<Duration> {
    let v = value.trim();
    let (num, unit) = if let Some(n) = v.strip_suffix("ms") {
        (n, "ms")
    } else if let Some(n) = v.strip_suffix('s') {
        (n, "s")
    } else if let Some(n) = v.strip_suffix('m') {
        (n, "m")
    } else if let Some(n) = v.strip_suffix('h') {
        (n, "h")
    } else {
        (v, "s") // bare number = seconds
    };
    let amount: u64 = num.trim().parse().ok()?;
    let dur = match unit {
        "ms" => Duration::from_millis(amount),
        "s" => Duration::from_secs(amount),
        "m" => Duration::from_secs(amount.checked_mul(60)?),
        "h" => Duration::from_secs(amount.checked_mul(3600)?),
        _ => return None,
    };
    Some(dur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Response, StatusCode};
    use kube::client::Body;
    use serde_json::json;
    use std::convert::Infallible;
    use std::sync::Arc;
    use tower::service_fn;

    fn empty_kubernetes_list(api_version: &str, kind: &str) -> Response<Body> {
        Response::builder()
            .status(StatusCode::OK)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "apiVersion": api_version,
                    "kind": kind,
                    "metadata": {"resourceVersion": "1"},
                    "items": []
                }))
                .expect("response JSON must serialize"),
            ))
            .expect("response must build")
    }

    #[tokio::test]
    async fn reconcile_lists_only_configured_namespaces() {
        let paths = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&paths);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let path = request.uri().path().to_string();
                captured.lock().push(path.clone());
                let response = if path.contains("endpointslices") {
                    empty_kubernetes_list("discovery.k8s.io/v1", "EndpointSliceList")
                } else {
                    empty_kubernetes_list("v1", "ServiceList")
                };
                Ok::<_, Infallible>(response)
            }
        });

        let client = Client::new(service, "default");
        reconcile_once(
            &client,
            &["team-a".into(), "team-b".into()],
            &DnsResolverConfig::default(),
            &mut HashMap::new(),
        )
        .await
        .expect("namespace-scoped reconcile must succeed");

        assert_eq!(
            *paths.lock(),
            vec![
                "/apis/discovery.k8s.io/v1/namespaces/team-a/endpointslices",
                "/apis/discovery.k8s.io/v1/namespaces/team-b/endpointslices",
                "/api/v1/namespaces/team-a/services",
                "/api/v1/namespaces/team-b/services"
            ]
        );
    }

    #[test]
    fn parse_refresh_interval_units() {
        assert_eq!(parse_duration_str("15s"), Some(Duration::from_secs(15)));
        assert_eq!(parse_duration_str("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration_str("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration_str("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration_str("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration_str("bogus"), None);
    }

    #[test]
    fn parse_refresh_interval_defaults_and_clamps() {
        assert_eq!(parse_refresh_interval("bogus"), DEFAULT_REFRESH); // unparseable → default
        assert_eq!(parse_refresh_interval("0s"), MIN_REFRESH); // zero clamped up
        assert_eq!(parse_refresh_interval("500ms"), MIN_REFRESH); // below min clamped up
        assert_eq!(parse_refresh_interval("15s"), Duration::from_secs(15));
    }

    #[test]
    fn can_skip_apply_requires_existence_and_matching_fingerprint() {
        let key = "app/ext".to_string();
        let fp = "10.0.0.1|80".to_string();
        let mut last_applied = HashMap::new();
        last_applied.insert(key.clone(), fp.clone());

        let mut observed = HashSet::new();
        observed.insert(key.clone());

        // Present + fingerprint matches → skip.
        assert!(can_skip_apply(&observed, &last_applied, &key, &fp));
        // Slice deleted out-of-band (not observed) → must NOT skip, even though the
        // fingerprint still matches (this is the regression guard).
        assert!(!can_skip_apply(&HashSet::new(), &last_applied, &key, &fp));
        // Present but fingerprint changed → must NOT skip.
        assert!(!can_skip_apply(&observed, &last_applied, &key, "10.0.0.2|80"));
        // Never applied before → must NOT skip.
        assert!(!can_skip_apply(&observed, &HashMap::new(), &key, &fp));
    }

    #[test]
    fn slice_fingerprint_stable_and_discriminating() {
        let a: Vec<IpAddr> = vec![IpAddr::from([10, 0, 0, 1]), IpAddr::from([10, 0, 0, 2])];
        let ports = |nums: &[i32]| {
            nums.iter()
                .map(|&p| ServicePort {
                    port: p,
                    ..Default::default()
                })
                .collect::<Vec<_>>()
        };
        let fp = slice_fingerprint(&a, &ports(&[80]));
        // Same inputs → same fingerprint (no spurious re-apply).
        assert_eq!(fp, slice_fingerprint(&a, &ports(&[80])));
        // A changed IP set or port set → different fingerprint (re-apply).
        assert_ne!(fp, slice_fingerprint(&[IpAddr::from([10, 0, 0, 1])], &ports(&[80])));
        assert_ne!(fp, slice_fingerprint(&a, &ports(&[80, 443])));
    }

    #[test]
    fn controller_default_candidate_prefers_ref_then_servers() {
        let ref_cfg = DnsResolverConfig {
            enabled: true,
            refresh_interval: "15s".to_string(),
            link_sys_ref: Some("dns/aliyun".to_string()),
            servers: Vec::new(),
        };
        assert_eq!(
            controller_default_candidate(&ref_cfg),
            Some(ResolverCandidate::HttpDnsRef(LinkSysKey {
                namespace: "dns".to_string(),
                name: "aliyun".to_string(),
            }))
        );

        let servers_cfg = DnsResolverConfig {
            enabled: true,
            refresh_interval: "15s".to_string(),
            link_sys_ref: None,
            servers: vec!["1.1.1.1".to_string()],
        };
        assert_eq!(
            controller_default_candidate(&servers_cfg),
            Some(ResolverCandidate::Servers(vec!["1.1.1.1".to_string()]))
        );

        assert_eq!(controller_default_candidate(&DnsResolverConfig::default()), None);
    }
}
