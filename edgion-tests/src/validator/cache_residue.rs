//! Cache Residue Validator
//!
//! Asserts that removable cache state in controller and gateway reaches zero
//! after `wipe-all`. Used as the final step of `run_integration.sh` to
//! detect leaks across all integration scenarios.
//! ResponseCache retains its admitted numeric infrastructure baseline; only
//! allocated-minus-infrastructure and pending/entries/reader pins must be zero.
//!
//! See spec: docs/superpowers/specs/2026-04-26-cache-residue-detection-design.md

use clap::Parser;
use edgion_resources::{resource_defs::ALL_RESOURCE_INFOS, ResourceKind};
use reqwest::Client;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::process;
use std::time::{Duration, Instant};

const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_WIPE_TIMEOUT_SECS: u64 = 60;
const WIPE_INVENTORY_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_TIMEOUT_SECS: u64 = 75;

const ROOT_CA_DESCRIPTION: &str = "Contains a CA bundle that can be used to verify the kube-apiserver when using internal endpoints such as the internal service IP or kubernetes.default.svc. No other usage is guaranteed across distributions of Kubernetes clusters.";

#[derive(Parser, Debug)]
#[command(name = "cache_residue")]
#[command(about = "Verify all caches are empty after wipe-all (leak detection)")]
struct Args {
    #[arg(long)]
    controller_addr: String,

    #[arg(long)]
    gateway_addr: String,

    /// Bearer token for the controller admin API. The controller's test-only
    /// diagnostic routes (wipe-all, store-stats) require standard auth.
    #[arg(long)]
    controller_token: Option<String>,

    /// Bearer token for the gateway admin API. Defaults from the
    /// EDGION_GATEWAY_ADMIN_TOKEN env var (exported by the test harness).
    #[arg(long, env = "EDGION_GATEWAY_ADMIN_TOKEN")]
    gateway_token: Option<String>,

    #[arg(long, default_value_t = false)]
    no_wipe: bool,

    /// Kubernetes-only staged cleanup; never invoke the concurrent wipe API.
    #[arg(long, conflicts_with = "no_wipe")]
    k8s_staged_wipe: bool,

    /// Exact live conf-sync leader Service retained until ordinary caches converge.
    #[arg(long, requires = "k8s_staged_wipe")]
    k8s_leader_service: Option<String>,

    /// Deadline for the single destructive wipe request, independent of polling.
    #[arg(long, default_value_t = DEFAULT_WIPE_TIMEOUT_SECS, value_parser = clap::value_parser!(u64).range(1..))]
    wipe_timeout_secs: u64,

    /// Report only Controller-reported kind counts before the destructive request.
    #[arg(long, default_value_t = false)]
    report_wipe_inventory: bool,

    /// Upper bound for resource propagation, task cancellation, and lease release.
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS)]
    timeout_secs: u64,

    #[arg(long, default_value = "500")]
    poll_interval_ms: u64,

    /// `warn` (default) — exit 0 even when residue is detected, just print report.
    /// `fail` — exit 1 on residue. exit 2 always reserved for infra errors.
    #[arg(long, default_value = "warn")]
    mode: String,

    /// Skip one side: `controller` or `gateway`.
    #[arg(long)]
    skip: Option<String>,

    /// Ignore one exact ConfigMap identity from the Controller storage count.
    /// Repeat for multiple Kubernetes-owned baseline objects. Values must use
    /// `namespace/name`; the validator still reports every other ConfigMap.
    #[arg(long, value_name = "NAMESPACE/NAME")]
    ignore_controller_configmap: Vec<String>,

    /// Ignore one exact Controller-owned conf-sync Secret after revalidating
    /// its reserved name, ownership label, type, and data-key shape on every
    /// poll. Repeat for the CA and gateway leaf identities.
    #[arg(long, value_name = "NAMESPACE/NAME")]
    ignore_controller_conf_sync_secret: Vec<String>,
}

#[derive(Debug, Default)]
struct Snapshot {
    /// Flat map of "scope.path.field" -> count, post-ignore-path filter.
    /// Only non-zero entries are stored.
    residue: BTreeMap<String, u64>,
    /// Sample keys per residue path (capped to 3).
    samples: BTreeMap<String, Vec<String>>,
}

impl Snapshot {
    fn is_empty(&self) -> bool {
        self.residue.is_empty()
    }
}

// Stat paths exempted from the residue check.
//
// - `controller.event_store.next_resource_version`: monotonic version
//   counter; never resets. Structural, not residue.
// - `controller.watch_objects.count`: count of live watch subscriptions; not
//   tied to a resource lifecycle. Structural.
//
// DO NOT add `gateway.tcp_routes.total_routes_in_tables` (added by the
// cross-kind audit specifically so L4 orphan residue is OBSERVABLE).
const IGNORE_PATHS: &[&str] = &[
    // Live bindings belong to startup-owned listeners, which survive resource wipe.
    // Expired bindings remain checked: the EGC publication path must prune them.
    "gateway.egc_bindings.count",
    // Metrics admission is lifetime history, not removable business state.
    // Unexpired events can legitimately outlive this gate's 75-second poll.
    // Overdue epochs and all contribution owners remain mandatory zero checks.
    "gateway.metrics.admission_count",
    "gateway.metrics.fixed_initialized_count",
    "gateway.metrics.live_epochs",
    "gateway.metrics.key_bytes",
    "controller.metrics.admission_count",
    "controller.metrics.fixed_initialized_count",
    "controller.metrics.live_epochs",
    "controller.metrics.key_bytes",
    "controller.event_store.next_resource_version",
    "controller.watch_objects.count",
];

enum ExitDecision {
    Ok,
    Residue,
    Error,
}

impl ExitDecision {
    fn code(&self) -> i32 {
        match self {
            ExitDecision::Ok => 0,
            ExitDecision::Residue => 1,
            ExitDecision::Error => 2,
        }
    }
}

fn decide_exit(snap_empty: bool, mode: &str) -> Result<ExitDecision, String> {
    if snap_empty {
        return Ok(ExitDecision::Ok);
    }
    match mode {
        "warn" => Ok(ExitDecision::Ok),
        "fail" => Ok(ExitDecision::Residue),
        other => Err(format!("invalid --mode {}, expected warn|fail", other)),
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let ignored_controller_configmaps = match parse_controller_configmap_ignores(&args.ignore_controller_configmap) {
        Ok(ignored) => ignored,
        Err(message) => {
            eprintln!("[ERROR] {message}");
            process::exit(2);
        }
    };
    let ignored_controller_conf_sync_secrets =
        match parse_controller_conf_sync_secret_ignores(&args.ignore_controller_conf_sync_secret) {
            Ok(ignored) => ignored,
            Err(message) => {
                eprintln!("[ERROR] {message}");
                process::exit(2);
            }
        };
    let client = Client::builder()
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client");

    if args.k8s_staged_wipe {
        if let Err(error) = staged_wipe(
            &client,
            &args,
            &ignored_controller_configmaps,
            &ignored_controller_conf_sync_secrets,
        )
        .await
        {
            eprintln!("[ERROR] staged cleanup failed: {error}");
            process::exit(2);
        }
        process::exit(0);
    }

    // Step 1: wipe-all (unless --no-wipe)
    if let Err(error) = wipe_and_report(&client, &args).await {
        eprintln!("[ERROR] wipe-all failed: {error}");
        process::exit(2);
    }

    // Step 2: clear access logs (gateway side, removes TTL false positive)
    if args.skip.as_deref() != Some("gateway") {
        let _ = clear_access_logs(&client, &args.gateway_addr, args.gateway_token.as_deref()).await;
    }

    // Step 3: poll until empty or timeout
    let deadline = Instant::now() + Duration::from_secs(args.timeout_secs);
    let last: Snapshot = loop {
        match collect(
            &client,
            &args,
            &ignored_controller_configmaps,
            &ignored_controller_conf_sync_secrets,
        )
        .await
        {
            Ok(snap) => {
                if snap.is_empty() {
                    eprintln!("[PASS] All checked caches are empty");
                    process::exit(0);
                }
                if Instant::now() >= deadline {
                    break snap;
                }
                tokio::time::sleep(Duration::from_millis(args.poll_interval_ms)).await;
            }
            Err(e) => {
                eprintln!("[ERROR] collect failed: {}", e);
                process::exit(2);
            }
        }
    };

    // Step 4: print report and exit per mode
    print_report(&last, args.timeout_secs);
    match decide_exit(false, &args.mode) {
        Ok(decision) => process::exit(decision.code()),
        Err(msg) => {
            eprintln!("[ERROR] {}", msg);
            process::exit(ExitDecision::Error.code());
        }
    }
}

/// Attach a Bearer token to an admin-bound request when one is configured.
/// Both controller and gateway admin APIs require auth on their diagnostic routes.
fn with_bearer(rb: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
    match token {
        Some(t) => rb.bearer_auth(t),
        None => rb,
    }
}

fn parse_wipe_inventory(body: &Value) -> Result<BTreeMap<String, u64>, String> {
    let object = body
        .pointer("/data/conf_center_storage")
        .and_then(Value::as_object)
        .ok_or("Controller-reported wipe inventory is missing its counts map")?;
    let expected = edgion_resources::resource_defs::ALL_RESOURCE_INFOS
        .iter()
        .map(|info| info.kind.as_str())
        .collect::<BTreeSet<_>>();
    if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err("Controller-reported wipe inventory does not match the canonical kind inventory".into());
    }
    let counts = object
        .iter()
        .map(|(kind, value)| {
            value
                .as_u64()
                .map(|count| (kind.clone(), count))
                .ok_or_else(|| "Controller-reported wipe inventory has a non-integer count".to_owned())
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    counts
        .values()
        .try_fold(0u64, |total, count| total.checked_add(*count))
        .ok_or("Controller-reported wipe inventory total overflows")?;
    Ok(counts)
}

async fn wipe_and_report(client: &Client, args: &Args) -> Result<(), String> {
    if args.no_wipe || args.skip.as_deref() == Some("controller") {
        return Ok(());
    }
    if args.report_wipe_inventory {
        let url = format!("{}/api/v1/debug/store-stats", args.controller_addr);
        let body = wipe_inventory_request(client, &url, args.controller_token.as_deref())
            .send()
            .await
            .map_err(|error| format!("wipe inventory request failed: {error}"))?
            .error_for_status()
            .map_err(|error| format!("wipe inventory request failed: {error}"))?
            .json::<Value>()
            .await
            .map_err(|_| "wipe inventory response was not JSON".to_owned())?;
        let counts = parse_wipe_inventory(&body)?;
        // The diagnostic handler reports zero on an individual list error.
        // These are Controller-reported counts, not an independent inventory proof.
        for (kind, count) in &counts {
            eprintln!("[INFO] Controller-reported wipe inventory: {kind}={count}");
        }
        eprintln!(
            "[INFO] Controller-reported wipe inventory: total={} wipe_timeout_secs={}",
            counts.values().sum::<u64>(),
            args.wipe_timeout_secs
        );
    }
    wipe_all(
        client,
        &args.controller_addr,
        args.controller_token.as_deref(),
        Duration::from_secs(args.wipe_timeout_secs),
    )
    .await
    .map_err(|error| error.to_string())
}

fn wipe_inventory_request(client: &Client, url: &str, token: Option<&str>) -> reqwest::RequestBuilder {
    with_bearer(client.get(url), token).timeout(WIPE_INVENTORY_REQUEST_TIMEOUT)
}

async fn wipe_all(client: &Client, controller: &str, token: Option<&str>, timeout: Duration) -> reqwest::Result<()> {
    let url = format!("{}/api/v1/admin/wipe-all", controller);
    // A Kubernetes wipe deletes hundreds of objects through the API server and
    // can legitimately exceed the short timeout used by read-only polling.
    // Keep one extended request instead of retrying an idempotent-but-still-
    // running destructive operation after an ambiguous transport timeout.
    let resp = wipe_all_request(client, &url, token, timeout)
        .send()
        .await?
        .error_for_status()?;
    let body: Value = resp.json().await?;
    eprintln!(
        "[INFO] wipe-all: total={} errors={} duration_ms={}",
        body.pointer("/data/total").and_then(|v| v.as_u64()).unwrap_or(0),
        body.pointer("/data/errors")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0),
        body.pointer("/data/duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
    );
    Ok(())
}

fn wipe_all_request(client: &Client, url: &str, token: Option<&str>, timeout: Duration) -> reqwest::RequestBuilder {
    with_bearer(client.post(url), token).timeout(timeout)
}

async fn clear_access_logs(client: &Client, gateway: &str, token: Option<&str>) -> reqwest::Result<()> {
    let url = format!("{}/api/v1/testing/access-logs", gateway);
    with_bearer(client.delete(&url), token)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn collect(
    client: &Client,
    args: &Args,
    ignored_controller_configmaps: &BTreeSet<String>,
    ignored_controller_conf_sync_secrets: &BTreeSet<String>,
) -> anyhow::Result<Snapshot> {
    let mut snap = Snapshot::default();

    if args.skip.as_deref() != Some("controller") {
        let url = format!("{}/api/v1/debug/store-stats", args.controller_addr);
        let body: Value = with_bearer(client.get(&url), args.controller_token.as_deref())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let data = require_store_stats_data(&body, "controller")?;
        let runtime_secret_keys = parse_secret_store_keys(data);
        walk_json("controller", data, &mut snap);
        if let Some(kinds) = data.get("raw_cache").and_then(Value::as_object) {
            for kind in kinds.keys() {
                apply_raw_fixture_exclusions(data, kind, &BTreeSet::new(), &mut snap);
            }
        }
        if !ignored_controller_conf_sync_secrets.is_empty() && runtime_secret_keys.is_none() {
            snap.residue.insert("controller.secret_store_contract".to_string(), 1);
        }

        if !ignored_controller_configmaps.is_empty() {
            if let Some(items) = fetch_controller_resources(client, args, "ConfigMap").await? {
                let verified = items
                    .iter()
                    .filter_map(|item| {
                        let key = resource_key(item)?;
                        (ignored_controller_configmaps.contains(&key) && is_verified_root_ca_configmap(item, &key))
                            .then_some(key)
                    })
                    .collect::<BTreeSet<_>>();
                let remaining_keys = resource_keys(&items)
                    .filter(|key| !verified.contains(key))
                    .collect::<Vec<_>>();
                apply_raw_fixture_exclusions(data, "ConfigMap", &verified, &mut snap);
                set_residue_count(
                    &mut snap,
                    "controller.conf_center_storage.ConfigMap",
                    remaining_keys.len() as u64,
                );
                if !remaining_keys.is_empty() {
                    snap.samples.insert(
                        "controller.conf_center_storage.ConfigMap".to_string(),
                        remaining_keys.into_iter().take(3).collect(),
                    );
                }
            }
        }

        if !ignored_controller_conf_sync_secrets.is_empty() {
            if let Some(items) = fetch_controller_resources(client, args, "Secret").await? {
                let verified = items
                    .iter()
                    .filter_map(|item| {
                        let key = resource_key(item)?;
                        (ignored_controller_conf_sync_secrets.contains(&key)
                            && is_verified_conf_sync_secret(item, &key))
                        .then_some(key)
                    })
                    .collect::<BTreeSet<_>>();
                let remaining_keys = resource_keys(&items)
                    .filter(|key| !verified.contains(key))
                    .collect::<Vec<_>>();
                set_residue_count(
                    &mut snap,
                    "controller.conf_center_storage.Secret",
                    remaining_keys.len() as u64,
                );
                if !remaining_keys.is_empty() {
                    snap.samples.insert(
                        "controller.conf_center_storage.Secret".to_string(),
                        remaining_keys.into_iter().take(3).collect(),
                    );
                }

                if let Some(runtime_keys) = &runtime_secret_keys {
                    let remaining_runtime_keys = runtime_keys.difference(&verified).cloned().collect::<Vec<_>>();
                    set_residue_count(
                        &mut snap,
                        "controller.secret_store",
                        remaining_runtime_keys.len() as u64,
                    );
                    if !remaining_runtime_keys.is_empty() {
                        snap.samples.insert(
                            "controller.secret_store".to_string(),
                            remaining_runtime_keys.into_iter().take(3).collect(),
                        );
                    }
                }
                apply_raw_fixture_exclusions(data, "Secret", &verified, &mut snap);
            }
        }
    }

    if args.skip.as_deref() != Some("gateway") {
        let url = format!("{}/api/v1/debug/store-stats", args.gateway_addr);
        let body: Value = with_bearer(client.get(&url), args.gateway_token.as_deref())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let data = require_store_stats_data(&body, "gateway")?;
        walk_json("gateway", data, &mut snap);
    }

    // Best-effort sample key collection
    for (path, _) in snap.residue.clone() {
        if snap.samples.contains_key(&path) {
            continue;
        }
        if let Some(samples) = fetch_samples(
            client,
            args,
            &path,
            ignored_controller_configmaps,
            ignored_controller_conf_sync_secrets,
        )
        .await
        {
            snap.samples.insert(path, samples);
        }
    }

    Ok(snap)
}

fn require_store_stats_data<'a>(body: &'a Value, source: &str) -> anyhow::Result<&'a Value> {
    match body.get("success").and_then(Value::as_bool) {
        Some(true) => {}
        Some(false) => anyhow::bail!("{source} store-stats API reported failure"),
        None => anyhow::bail!("{source} store-stats response omitted boolean success"),
    }
    match body.get("data") {
        Some(data @ Value::Object(_)) => Ok(data),
        Some(_) => anyhow::bail!("{source} store-stats response data must be an object"),
        None => anyhow::bail!("{source} store-stats success response omitted data"),
    }
}

async fn fetch_samples(
    client: &Client,
    args: &Args,
    path: &str,
    ignored_controller_configmaps: &BTreeSet<String>,
    ignored_controller_conf_sync_secrets: &BTreeSet<String>,
) -> Option<Vec<String>> {
    if let Some(rest) = path.strip_prefix("controller.conf_center_storage.") {
        let items = fetch_controller_resources(client, args, rest).await.ok()??;
        let names: Vec<String> = items
            .iter()
            .filter_map(|item| {
                let key = resource_key(item)?;
                let ignored_configmap = rest == "ConfigMap"
                    && ignored_controller_configmaps.contains(&key)
                    && is_verified_root_ca_configmap(item, &key);
                let ignored_secret = rest == "Secret"
                    && ignored_controller_conf_sync_secrets.contains(&key)
                    && is_verified_conf_sync_secret(item, &key);
                if ignored_configmap || ignored_secret {
                    None
                } else {
                    Some(key)
                }
            })
            .take(3)
            .collect();
        return Some(names);
    }
    if let Some(rest) = path.strip_prefix("gateway.client_caches.by_kind.") {
        let url = format!("{}/configclient/{}/list", args.gateway_addr, rest);
        let body: Value = with_bearer(client.get(&url), args.gateway_token.as_deref())
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let items = body.get("data")?.as_array()?;
        let names: Vec<String> = items
            .iter()
            .filter_map(|i| {
                let ns = i.pointer("/metadata/namespace").and_then(|v| v.as_str()).unwrap_or("");
                let n = i.pointer("/metadata/name").and_then(|v| v.as_str())?;
                Some(if ns.is_empty() {
                    n.to_string()
                } else {
                    format!("{}/{}", ns, n)
                })
            })
            .take(3)
            .collect();
        return Some(names);
    }
    None
}

async fn fetch_controller_resources(client: &Client, args: &Args, kind: &str) -> reqwest::Result<Option<Vec<Value>>> {
    let url = format!("{}/api/v1/namespaced/{}", args.controller_addr, kind);
    let body: Value = with_bearer(client.get(&url), args.controller_token.as_deref())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(body.pointer("/data").and_then(Value::as_array).cloned())
}

fn resource_key(item: &Value) -> Option<String> {
    let namespace = item
        .pointer("/metadata/namespace")
        .and_then(Value::as_str)
        .unwrap_or("");
    let name = item.pointer("/metadata/name").and_then(Value::as_str)?;
    Some(if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{namespace}/{name}")
    })
}

fn resource_keys(items: &[Value]) -> impl Iterator<Item = String> + '_ {
    items.iter().filter_map(resource_key)
}

fn parse_controller_configmap_ignores(values: &[String]) -> Result<BTreeSet<String>, String> {
    let ignored = parse_namespaced_keys(values, "--ignore-controller-configmap")?;
    for key in &ignored {
        if key.split_once('/').map(|(_, name)| name) != Some("kube-root-ca.crt") {
            return Err(format!(
                "invalid --ignore-controller-configmap {key:?}; only kube-root-ca.crt identities are accepted"
            ));
        }
    }
    Ok(ignored)
}

fn parse_controller_conf_sync_secret_ignores(values: &[String]) -> Result<BTreeSet<String>, String> {
    let ignored = parse_namespaced_keys(values, "--ignore-controller-conf-sync-secret")?;
    for key in &ignored {
        let name = key.split_once('/').map(|(_, name)| name).unwrap_or_default();
        if !matches!(name, "edgion-conf-sync-ca" | "edgion-conf-sync-certs") {
            return Err(format!(
                "invalid --ignore-controller-conf-sync-secret {key:?}; only reserved conf-sync Secret names are accepted"
            ));
        }
    }
    Ok(ignored)
}

fn parse_namespaced_keys(values: &[String], option: &str) -> Result<BTreeSet<String>, String> {
    let mut parsed = BTreeSet::new();
    for value in values {
        let mut parts = value.split('/');
        let namespace = parts.next().unwrap_or_default();
        let name = parts.next().unwrap_or_default();
        if namespace.is_empty() || name.is_empty() || parts.next().is_some() {
            return Err(format!("invalid {option} {value:?}; expected exact namespace/name"));
        }
        parsed.insert(value.clone());
    }
    Ok(parsed)
}

fn is_verified_conf_sync_secret(item: &Value, key: &str) -> bool {
    if resource_key(item).as_deref() != Some(key)
        || item
            .pointer("/metadata/labels/edgion.io~1managed-by")
            .and_then(Value::as_str)
            != Some("conf-sync-ca")
        || item.get("type").and_then(Value::as_str) != Some("Opaque")
    {
        return false;
    }

    let Some((_, name)) = key.split_once('/') else {
        return false;
    };
    let expected = match name {
        "edgion-conf-sync-ca" => BTreeSet::from(["ca.crt", "ca.key"]),
        "edgion-conf-sync-certs" => BTreeSet::from(["ca.crt", "client.crt", "client.key"]),
        _ => return false,
    };
    let Some(data) = item.get("data").and_then(Value::as_object) else {
        return false;
    };
    let actual = data.keys().map(String::as_str).collect::<BTreeSet<_>>();
    actual == expected
        && data
            .values()
            .all(|value| value.as_str().is_some_and(|value| !value.is_empty()))
}

fn is_verified_root_ca_configmap(item: &Value, key: &str) -> bool {
    if resource_key(item).as_deref() != Some(key)
        || key.split_once('/').map(|(_, name)| name) != Some("kube-root-ca.crt")
        || item
            .pointer("/metadata/annotations/kubernetes.io~1description")
            .and_then(Value::as_str)
            != Some(ROOT_CA_DESCRIPTION)
        || item
            .get("binaryData")
            .and_then(Value::as_object)
            .is_some_and(|data| !data.is_empty())
    {
        return false;
    }
    let Some(data) = item.get("data").and_then(Value::as_object) else {
        return false;
    };
    data.len() == 1
        && data
            .get("ca.crt")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
}

fn parse_secret_store_keys(data: &Value) -> Option<BTreeSet<String>> {
    let reported = data.get("secret_store")?.as_u64()? as usize;
    let values = data.get("secret_store_keys")?.as_array()?;
    let keys = values
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    (keys.len() == values.len() && keys.len() == reported).then_some(keys)
}

fn set_residue_count(snap: &mut Snapshot, path: &str, count: u64) {
    if count == 0 {
        snap.residue.remove(path);
        snap.samples.remove(path);
    } else {
        snap.residue.insert(path.to_string(), count);
    }
}

/// Walk a JSON object recursively. For every numeric leaf, if its full path
/// is not in IGNORE_PATHS and value > 0, record it.
/// Exclude only identities verified against the existing API-side fixture profile.
/// Never subtract API counts from a different raw cache or ignore the whole bucket.
fn apply_raw_fixture_exclusions(data: &Value, kind: &str, verified: &BTreeSet<String>, snap: &mut Snapshot) {
    let Some(count) = data.get("raw_cache").and_then(|v| v.get(kind)).and_then(Value::as_u64) else {
        return;
    };
    let path = format!("controller.raw_cache.{kind}");
    let keys = data
        .get("raw_cache_keys")
        .and_then(|v| v.get(kind))
        .and_then(Value::as_array)
        .and_then(|values| {
            values
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<BTreeSet<_>>>()
        });
    let Some(keys) = keys.filter(|keys| keys.len() as u64 == count) else {
        snap.residue.insert(format!("controller.raw_cache_contract.{kind}"), 1);
        return;
    };
    let remaining = keys.difference(verified).cloned().collect::<Vec<_>>();
    set_residue_count(snap, &path, remaining.len() as u64);
    snap.samples.remove(&path);
    if !remaining.is_empty() {
        snap.samples.insert(path, remaining.into_iter().take(3).collect());
    }
}

fn walk_json(prefix: &str, value: &Value, snap: &mut Snapshot) {
    if prefix == "gateway.response_cache" {
        let Some(map) = value.as_object() else {
            snap.residue.insert("gateway.response_cache.contract".into(), 1);
            return;
        };
        if map.len() != 5 {
            snap.residue.insert("gateway.response_cache.contract".into(), 1);
            return;
        }
        let number = |name: &str| map.get(name).and_then(Value::as_u64);
        let Some((allocated, infrastructure)) = number("allocated_bytes").zip(number("infrastructure_bytes")) else {
            snap.residue.insert("gateway.response_cache.contract".into(), 1);
            return;
        };
        match allocated.checked_sub(infrastructure) {
            Some(0) => {}
            Some(removable) => {
                snap.residue
                    .insert("gateway.response_cache.removable_bytes".into(), removable);
            }
            None => {
                snap.residue.insert("gateway.response_cache.contract".into(), 1);
            }
        }
        for name in ["pending_bytes", "entries", "reader_pins"] {
            match number(name) {
                Some(0) => {}
                Some(count) => {
                    snap.residue.insert(format!("gateway.response_cache.{name}"), count);
                }
                None => {
                    snap.residue.insert("gateway.response_cache.contract".into(), 1);
                }
            }
        }
        return;
    }
    match value {
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                if u > 0 && !IGNORE_PATHS.contains(&prefix) {
                    snap.residue.insert(prefix.to_string(), u);
                }
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let path = format!("{}.{}", prefix, k);
                walk_json(&path, v, snap);
            }
        }
        Value::Array(_) | Value::String(_) | Value::Bool(_) | Value::Null => {}
    }
}

fn print_report(snap: &Snapshot, timeout_secs: u64) {
    eprintln!();
    eprintln!(
        "[FAIL] Cache residue detected after wipe-all (timeout {}s):",
        timeout_secs
    );
    eprintln!();
    let mut by_scope: BTreeMap<&str, Vec<(&String, &u64)>> = BTreeMap::new();
    for (path, count) in &snap.residue {
        let scope = path.split('.').next().unwrap_or("?");
        by_scope.entry(scope).or_default().push((path, count));
    }
    for (scope, entries) in by_scope {
        eprintln!("  {}:", scope);
        for (path, count) in entries {
            let stripped = path.strip_prefix(&format!("{}.", scope)).unwrap_or(path);
            let sample = snap
                .samples
                .get(path)
                .map(|s| format!("   sample: {:?}", s))
                .unwrap_or_default();
            eprintln!("    {:50} = {}{}", stripped, count, sample);
        }
    }
    eprintln!();
    let total: u64 = snap.residue.values().sum();
    eprintln!("Total residue: {} entries across {} buckets", total, snap.residue.len());
}

#[cfg(test)]
mod exit_tests {
    use super::*;

    #[test]
    fn default_timeout_exceeds_wasm_eviction_grace() {
        let args = Args::try_parse_from([
            "cache_residue",
            "--controller-addr",
            "http://controller",
            "--gateway-addr",
            "http://gateway",
        ])
        .expect("required addresses are present");
        assert_eq!(args.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(args.timeout_secs > 60);
        assert_eq!(args.wipe_timeout_secs, 60);
        assert!(!args.report_wipe_inventory);
    }

    fn reported_inventory() -> Value {
        let counts = edgion_resources::resource_defs::ALL_RESOURCE_INFOS
            .iter()
            .map(|info| (info.kind.as_str().to_owned(), Value::from(1u64)))
            .collect::<serde_json::Map<_, _>>();
        serde_json::json!({"data": {
            "conf_center_storage": counts,
            "other": {"secret": "must not enter the count report"}
        }})
    }

    fn inventory_args(controller: &str) -> Args {
        Args::try_parse_from([
            "cache_residue",
            "--controller-addr",
            controller,
            "--gateway-addr",
            "http://gateway",
            "--wipe-timeout-secs",
            "300",
            "--report-wipe-inventory",
        ])
        .unwrap()
    }

    #[test]
    fn independent_wipe_deadline_preserves_polling_and_rejects_zero() {
        let args = inventory_args("http://controller");
        assert_eq!(args.wipe_timeout_secs, 300);
        assert_eq!(args.timeout_secs, 75);
        assert!(Args::try_parse_from([
            "cache_residue",
            "--controller-addr",
            "http://controller",
            "--gateway-addr",
            "http://gateway",
            "--wipe-timeout-secs",
            "0"
        ])
        .is_err());
    }

    #[test]
    fn wipe_inventory_parser_keeps_only_canonical_unsigned_counts() {
        let body = reported_inventory();
        let counts = parse_wipe_inventory(&body).unwrap();
        assert_eq!(counts.len(), edgion_resources::resource_defs::ALL_RESOURCE_INFOS.len());
        assert!(counts.values().all(|count| *count == 1));
        assert!(!serde_json::to_string(&counts).unwrap().contains("secret"));
        assert!(parse_wipe_inventory(&serde_json::json!({})).is_err());
        for value in [Value::from(-1), Value::from("1"), Value::from(1.5), Value::Null] {
            let mut malformed = body.clone();
            malformed["data"]["conf_center_storage"]["Secret"] = value;
            assert!(parse_wipe_inventory(&malformed).is_err());
        }
        let mut missing = body.clone();
        missing["data"]["conf_center_storage"]
            .as_object_mut()
            .unwrap()
            .remove("Secret");
        assert!(parse_wipe_inventory(&missing).is_err());
        let mut overflow = body;
        overflow["data"]["conf_center_storage"]["Secret"] = Value::from(u64::MAX);
        assert!(parse_wipe_inventory(&overflow).is_err());
    }

    #[tokio::test]
    async fn wipe_inventory_failure_prevents_the_destructive_request() {
        use wiremock::{
            matchers::{method, path},
            Mock, MockServer, ResponseTemplate,
        };
        let mut malformed = reported_inventory();
        malformed["data"]["conf_center_storage"]["Secret"] = Value::from(-1);
        for (status, body) in [
            (200, serde_json::json!({})),
            (200, malformed),
            (500, reported_inventory()),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/v1/debug/store-stats"))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/api/v1/admin/wipe-all"))
                .respond_with(ResponseTemplate::new(200))
                .expect(0)
                .mount(&server)
                .await;
            assert!(wipe_and_report(&Client::new(), &inventory_args(&server.uri()))
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn skipped_wipe_does_not_request_an_inventory() {
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        for verb in ["GET", "POST"] {
            Mock::given(method(verb))
                .respond_with(ResponseTemplate::new(500))
                .expect(0)
                .mount(&server)
                .await;
        }
        let mut args = inventory_args(&server.uri());
        args.no_wipe = true;
        assert!(wipe_and_report(&Client::new(), &args).await.is_ok());
        args.no_wipe = false;
        args.skip = Some("controller".into());
        assert!(wipe_and_report(&Client::new(), &args).await.is_ok());
    }

    #[tokio::test]
    async fn wipe_failure_does_not_retry_the_destructive_request() {
        use wiremock::{
            matchers::{method, path},
            Mock, MockServer, ResponseTemplate,
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/debug/store-stats"))
            .respond_with(ResponseTemplate::new(200).set_body_json(reported_inventory()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/admin/wipe-all"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        assert!(wipe_and_report(&Client::new(), &inventory_args(&server.uri()))
            .await
            .is_err());
    }

    #[test]
    fn response_cache_residue_subtracts_only_numeric_infrastructure_and_preserves_retired_leases() {
        let mut clean = Snapshot::default();
        walk_json(
            "gateway",
            &serde_json::json!({"response_cache": {
                "infrastructure_bytes": 8192, "allocated_bytes": 8192,
                "pending_bytes": 0, "entries": 0, "reader_pins": 0
            }}),
            &mut clean,
        );
        assert!(clean.residue.is_empty());
        let mut retained = Snapshot::default();
        walk_json(
            "gateway",
            &serde_json::json!({"response_cache": {
                "infrastructure_bytes": 8192, "allocated_bytes": 8300,
                "pending_bytes": 12, "entries": 1, "reader_pins": 2
            }}),
            &mut retained,
        );
        assert_eq!(retained.residue["gateway.response_cache.removable_bytes"], 108);
        assert_eq!(retained.residue["gateway.response_cache.pending_bytes"], 12);
        assert_eq!(retained.residue["gateway.response_cache.entries"], 1);
        assert_eq!(retained.residue["gateway.response_cache.reader_pins"], 2);
        for value in [
            serde_json::json!({"allocated_bytes": 0, "infrastructure_bytes": 1}),
            serde_json::json!({"allocated_bytes": "8192", "infrastructure_bytes": 8192}),
            serde_json::json!({}),
        ] {
            let mut malformed = Snapshot::default();
            walk_json("gateway.response_cache", &value, &mut malformed);
            assert_eq!(malformed.residue["gateway.response_cache.contract"], 1);
        }
    }

    #[test]
    fn empty_snapshot_always_exits_zero() {
        assert!(matches!(decide_exit(true, "warn").unwrap(), ExitDecision::Ok));
        assert!(matches!(decide_exit(true, "fail").unwrap(), ExitDecision::Ok));
    }

    #[test]
    fn residue_warn_exits_zero() {
        assert!(matches!(decide_exit(false, "warn").unwrap(), ExitDecision::Ok));
    }

    #[test]
    fn residue_fail_exits_one() {
        assert!(matches!(decide_exit(false, "fail").unwrap(), ExitDecision::Residue));
    }

    #[test]
    fn invalid_mode_errors() {
        assert!(decide_exit(false, "bogus").is_err());
    }
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn collect_with_residue_returns_nonempty_snapshot() {
        let ctrl = MockServer::start().await;
        let gw = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/v1/debug/store-stats"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": { "secret_store": 2, "event_store": { "next_resource_version": 5 } }
            })))
            .mount(&ctrl)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/debug/store-stats"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": { "service_discovery": { "service_sources": 1, "service_keys": ["default/backend"] } }
            })))
            .mount(&gw)
            .await;

        let args = Args {
            controller_addr: ctrl.uri(),
            gateway_addr: gw.uri(),
            controller_token: None,
            gateway_token: None,
            no_wipe: true,
            k8s_staged_wipe: false,
            k8s_leader_service: None,
            wipe_timeout_secs: DEFAULT_WIPE_TIMEOUT_SECS,
            report_wipe_inventory: false,
            timeout_secs: 1,
            poll_interval_ms: 100,
            mode: "warn".to_string(),
            skip: None,
            ignore_controller_configmap: Vec::new(),
            ignore_controller_conf_sync_secret: Vec::new(),
        };
        let client = Client::new();
        let snap = collect(&client, &args, &BTreeSet::new(), &BTreeSet::new())
            .await
            .unwrap();
        assert_eq!(snap.residue.get("controller.secret_store"), Some(&2));
        assert_eq!(snap.residue.get("gateway.service_discovery.service_sources"), Some(&1));
        assert!(!snap
            .residue
            .contains_key("controller.event_store.next_resource_version"));
    }

    #[tokio::test]
    async fn collect_preserves_live_wasm_shared_data_metric_residue() {
        let gw = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/v1/debug/store-stats"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": {
                    "metrics": {
                        "admission_count": 10,
                        "fixed_initialized_count": 4,
                        "live_epochs": 6,
                        "key_bytes": 100,
                        "gauge_owners": 1,
                        "overdue_epochs": 1,
                        "keys": ["edgion_wasm_shared_data_bytes"]
                    }
                }
            })))
            .mount(&gw)
            .await;

        let args = Args {
            controller_addr: String::new(),
            gateway_addr: gw.uri(),
            controller_token: None,
            gateway_token: None,
            no_wipe: true,
            k8s_staged_wipe: false,
            k8s_leader_service: None,
            wipe_timeout_secs: DEFAULT_WIPE_TIMEOUT_SECS,
            report_wipe_inventory: false,
            timeout_secs: 1,
            poll_interval_ms: 100,
            mode: "fail".to_string(),
            skip: Some("controller".to_string()),
            ignore_controller_configmap: Vec::new(),
            ignore_controller_conf_sync_secret: Vec::new(),
        };
        let snap = collect(&Client::new(), &args, &BTreeSet::new(), &BTreeSet::new())
            .await
            .unwrap();

        assert_eq!(snap.residue.get("gateway.metrics.gauge_owners"), Some(&1));
        assert_eq!(snap.residue.get("gateway.metrics.overdue_epochs"), Some(&1));
        assert!(!snap.residue.contains_key("gateway.metrics.admission_count"));
        assert!(!snap.residue.contains_key("gateway.metrics.live_epochs"));
        assert!(matches!(
            decide_exit(snap.is_empty(), &args.mode).unwrap(),
            ExitDecision::Residue
        ));
    }

    #[test]
    fn store_stats_envelope_must_be_successful_and_contain_object_data() {
        for (body, expected) in [
            (json!({"success": false, "data": {}}), "reported failure"),
            (json!({"data": {}}), "omitted boolean success"),
            (json!({"success": true}), "omitted data"),
            (json!({"success": true, "data": null}), "data must be an object"),
            (json!({"success": true, "data": []}), "data must be an object"),
        ] {
            let error = require_store_stats_data(&body, "gateway")
                .expect_err("an incomplete store-stats check must fail closed");
            assert!(error.to_string().contains(expected), "unexpected error: {error}");
        }

        assert_eq!(
            require_store_stats_data(&json!({"success": true, "data": {}}), "gateway").unwrap(),
            &json!({})
        );
    }

    #[tokio::test]
    async fn collect_ignores_only_exact_controller_configmap_keys() {
        let ctrl = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/v1/debug/store-stats"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": { "conf_center_storage": { "ConfigMap": 3 }, "secret_store": 0, "secret_store_keys": [] }
            })))
            .mount(&ctrl)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/namespaced/ConfigMap"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": [
                    {
                        "metadata": {
                            "namespace": "app",
                            "name": "kube-root-ca.crt",
                            "annotations": { "kubernetes.io/description": ROOT_CA_DESCRIPTION }
                        },
                        "data": { "ca.crt": "test-ca" }
                    },
                    {
                        "metadata": {
                            "namespace": "app2",
                            "name": "kube-root-ca.crt",
                            "annotations": { "kubernetes.io/description": "not Kubernetes-owned" }
                        },
                        "data": { "ca.crt": "test-ca" }
                    },
                    { "metadata": { "namespace": "test", "name": "leaked" } }
                ]
            })))
            .mount(&ctrl)
            .await;

        let args = Args {
            controller_addr: ctrl.uri(),
            gateway_addr: String::new(),
            controller_token: None,
            gateway_token: None,
            no_wipe: true,
            k8s_staged_wipe: false,
            k8s_leader_service: None,
            wipe_timeout_secs: DEFAULT_WIPE_TIMEOUT_SECS,
            report_wipe_inventory: false,
            timeout_secs: 1,
            poll_interval_ms: 100,
            mode: "warn".to_string(),
            skip: Some("gateway".to_string()),
            ignore_controller_configmap: vec!["app/kube-root-ca.crt".to_string()],
            ignore_controller_conf_sync_secret: Vec::new(),
        };
        let ignored = parse_controller_configmap_ignores(&args.ignore_controller_configmap).unwrap();
        let snap = collect(&Client::new(), &args, &ignored, &BTreeSet::new())
            .await
            .unwrap();

        assert_eq!(snap.residue.get("controller.conf_center_storage.ConfigMap"), Some(&2));
        assert_eq!(
            snap.samples.get("controller.conf_center_storage.ConfigMap"),
            Some(&vec!["app2/kube-root-ca.crt".to_string(), "test/leaked".to_string()])
        );
    }

    #[tokio::test]
    async fn collect_revalidates_conf_sync_secrets_and_preserves_other_residue() {
        let ctrl = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/v1/debug/store-stats"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": {
                    "conf_center_storage": { "Secret": 3 },
                    "secret_store": 3,
                    "secret_store_keys": [
                        "edgion-system/edgion-conf-sync-ca",
                        "edgion-system/edgion-conf-sync-certs",
                        "test/leaked"
                    ]
                }
            })))
            .mount(&ctrl)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaced/Secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": [
                    {
                        "metadata": {
                            "namespace": "edgion-system",
                            "name": "edgion-conf-sync-ca",
                            "labels": { "edgion.io/managed-by": "conf-sync-ca" }
                        },
                        "type": "Opaque",
                        "data": { "ca.crt": "Y2E=", "ca.key": "a2V5" }
                    },
                    {
                        "metadata": {
                            "namespace": "edgion-system",
                            "name": "edgion-conf-sync-certs",
                            "labels": { "edgion.io/managed-by": "conf-sync-ca" }
                        },
                        "type": "Opaque",
                        "data": { "ca.crt": "Y2E=", "client.crt": "Y3J0", "client.key": "a2V5" }
                    },
                    { "metadata": { "namespace": "test", "name": "leaked" }, "type": "Opaque", "data": {} }
                ]
            })))
            .mount(&ctrl)
            .await;

        let secret_keys = BTreeSet::from([
            "edgion-system/edgion-conf-sync-ca".to_string(),
            "edgion-system/edgion-conf-sync-certs".to_string(),
        ]);
        let args = Args {
            controller_addr: ctrl.uri(),
            gateway_addr: String::new(),
            controller_token: None,
            gateway_token: None,
            no_wipe: true,
            k8s_staged_wipe: false,
            k8s_leader_service: None,
            wipe_timeout_secs: DEFAULT_WIPE_TIMEOUT_SECS,
            report_wipe_inventory: false,
            timeout_secs: 1,
            poll_interval_ms: 100,
            mode: "warn".to_string(),
            skip: Some("gateway".to_string()),
            ignore_controller_configmap: Vec::new(),
            ignore_controller_conf_sync_secret: secret_keys.iter().cloned().collect(),
        };

        let snap = collect(&Client::new(), &args, &BTreeSet::new(), &secret_keys)
            .await
            .unwrap();
        assert_eq!(snap.residue.get("controller.conf_center_storage.Secret"), Some(&1));
        assert_eq!(snap.residue.get("controller.secret_store"), Some(&1));
        assert_eq!(
            snap.samples.get("controller.conf_center_storage.Secret"),
            Some(&vec!["test/leaked".to_string()])
        );
        assert_eq!(
            snap.samples.get("controller.secret_store"),
            Some(&vec!["test/leaked".to_string()])
        );
    }

    #[tokio::test]
    async fn runtime_secret_keys_prevent_cross_store_baseline_subtraction() {
        let ctrl = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/debug/store-stats"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": {
                    "conf_center_storage": { "Secret": 2 },
                    "secret_store": 1,
                    "secret_store_keys": ["test/stale"]
                }
            })))
            .mount(&ctrl)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaced/Secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": [
                    {
                        "metadata": {
                            "namespace": "edgion-system",
                            "name": "edgion-conf-sync-ca",
                            "labels": { "edgion.io/managed-by": "conf-sync-ca" }
                        },
                        "type": "Opaque",
                        "data": { "ca.crt": "Y2E=", "ca.key": "a2V5" }
                    },
                    {
                        "metadata": {
                            "namespace": "edgion-system",
                            "name": "edgion-conf-sync-certs",
                            "labels": { "edgion.io/managed-by": "conf-sync-ca" }
                        },
                        "type": "Opaque",
                        "data": { "ca.crt": "Y2E=", "client.crt": "Y3J0", "client.key": "a2V5" }
                    }
                ]
            })))
            .mount(&ctrl)
            .await;

        let secret_keys = BTreeSet::from([
            "edgion-system/edgion-conf-sync-ca".to_string(),
            "edgion-system/edgion-conf-sync-certs".to_string(),
        ]);
        let args = Args {
            controller_addr: ctrl.uri(),
            gateway_addr: String::new(),
            controller_token: None,
            gateway_token: None,
            no_wipe: true,
            k8s_staged_wipe: false,
            k8s_leader_service: None,
            wipe_timeout_secs: DEFAULT_WIPE_TIMEOUT_SECS,
            report_wipe_inventory: false,
            timeout_secs: 1,
            poll_interval_ms: 100,
            mode: "warn".to_string(),
            skip: Some("gateway".to_string()),
            ignore_controller_configmap: Vec::new(),
            ignore_controller_conf_sync_secret: secret_keys.iter().cloned().collect(),
        };

        let snap = collect(&Client::new(), &args, &BTreeSet::new(), &secret_keys)
            .await
            .unwrap();
        assert!(!snap.residue.contains_key("controller.conf_center_storage.Secret"));
        assert_eq!(snap.residue.get("controller.secret_store"), Some(&1));
        assert_eq!(
            snap.samples.get("controller.secret_store"),
            Some(&vec!["test/stale".to_string()])
        );
    }

    #[tokio::test]
    async fn malformed_runtime_secret_key_contract_fails_closed() {
        let ctrl = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/debug/store-stats"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": {
                    "conf_center_storage": { "Secret": 0 },
                    "secret_store": 0,
                    "secret_store_keys": ["test/stale"]
                }
            })))
            .mount(&ctrl)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaced/Secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": []
            })))
            .mount(&ctrl)
            .await;

        let secret_keys = BTreeSet::from([
            "edgion-system/edgion-conf-sync-ca".to_string(),
            "edgion-system/edgion-conf-sync-certs".to_string(),
        ]);
        let args = Args {
            controller_addr: ctrl.uri(),
            gateway_addr: String::new(),
            controller_token: None,
            gateway_token: None,
            no_wipe: true,
            k8s_staged_wipe: false,
            k8s_leader_service: None,
            wipe_timeout_secs: DEFAULT_WIPE_TIMEOUT_SECS,
            report_wipe_inventory: false,
            timeout_secs: 1,
            poll_interval_ms: 100,
            mode: "warn".to_string(),
            skip: Some("gateway".to_string()),
            ignore_controller_configmap: Vec::new(),
            ignore_controller_conf_sync_secret: secret_keys.iter().cloned().collect(),
        };

        let snap = collect(&Client::new(), &args, &BTreeSet::new(), &secret_keys)
            .await
            .unwrap();
        assert_eq!(snap.residue.get("controller.secret_store_contract"), Some(&1));
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn raw_cache_exclusions_use_exact_verified_keys_and_reject_inconsistent_counts() {
        let mut snap = Snapshot::default();
        let data = json!({"raw_cache":{"Secret":2},"raw_cache_keys":{"Secret":["system/fixture","test/stale"]}});
        apply_raw_fixture_exclusions(&data, "Secret", &BTreeSet::from(["system/fixture".into()]), &mut snap);
        assert_eq!(snap.residue["controller.raw_cache.Secret"], 1);
        assert_eq!(snap.samples["controller.raw_cache.Secret"], vec!["test/stale"]);
        apply_raw_fixture_exclusions(
            &json!({"raw_cache":{"Secret":0},"raw_cache_keys":{"Secret":[]}}),
            "Secret",
            &BTreeSet::new(),
            &mut snap,
        );
        assert!(!snap.residue.contains_key("controller.raw_cache.Secret"));
        apply_raw_fixture_exclusions(
            &json!({"raw_cache":{"Secret":2},"raw_cache_keys":{"Secret":["system/fixture"]}}),
            "Secret",
            &BTreeSet::new(),
            &mut snap,
        );
        assert_eq!(snap.residue["controller.raw_cache_contract.Secret"], 1);
    }

    #[test]
    fn wipe_all_overrides_the_short_polling_request_timeout() {
        let client = Client::builder().timeout(HTTP_REQUEST_TIMEOUT).build().unwrap();
        let request = wipe_all_request(
            &client,
            "http://127.0.0.1/wipe",
            None,
            Duration::from_secs(DEFAULT_WIPE_TIMEOUT_SECS),
        )
        .build()
        .unwrap();

        assert_eq!(request.timeout(), Some(&Duration::from_secs(60)));
        let request = wipe_all_request(&client, "http://127.0.0.1/wipe", None, Duration::from_secs(300))
            .build()
            .unwrap();
        assert_eq!(request.timeout(), Some(&Duration::from_secs(300)));
        let request = wipe_inventory_request(&client, "http://127.0.0.1/stats", None)
            .build()
            .unwrap();
        assert_eq!(request.timeout(), Some(&Duration::from_secs(60)));
        assert_eq!(HTTP_REQUEST_TIMEOUT, Duration::from_secs(10));
    }

    #[test]
    fn ignore_path_filters_event_store() {
        let mut snap = Snapshot::default();
        let v = json!({ "event_store": { "next_resource_version": 142 } });
        walk_json("controller", &v, &mut snap);
        assert!(snap.is_empty(), "ignored path must not appear");
    }

    #[test]
    fn egc_binding_residue_exempts_live_handles_but_detects_expired_entries() {
        let mut snap = Snapshot::default();
        walk_json(
            "gateway",
            &json!({"egc_bindings": {"count": 2, "expired_count": 1}}),
            &mut snap,
        );
        assert_eq!(snap.residue.get("gateway.egc_bindings.expired_count"), Some(&1));
        assert!(!snap.residue.contains_key("gateway.egc_bindings.count"));
        let mut clean = Snapshot::default();
        walk_json(
            "gateway",
            &json!({"egc_bindings": {"count": 1, "expired_count": 0}}),
            &mut clean,
        );
        assert!(clean.is_empty());
    }

    #[test]
    fn nonzero_leaf_recorded() {
        let mut snap = Snapshot::default();
        let v = json!({ "secret_store": 3, "endpoint_store": { "lb_count": 1, "ep_data_count": 0 } });
        walk_json("controller", &v, &mut snap);
        assert_eq!(snap.residue.get("controller.secret_store"), Some(&3));
        assert_eq!(snap.residue.get("controller.endpoint_store.lb_count"), Some(&1));
        assert!(!snap.residue.contains_key("controller.endpoint_store.ep_data_count"));
    }

    #[test]
    fn zero_values_skipped() {
        let mut snap = Snapshot::default();
        let v = json!({ "service_discovery": { "service_sources": 0, "service_keys": [] } });
        walk_json("gateway", &v, &mut snap);
        assert!(snap.is_empty());
    }

    #[test]
    fn retained_degrade_metric_owner_is_residue_until_the_last_snapshot_releases() {
        let mut retained = Snapshot::default();
        walk_json(
            "gateway",
            &json!({ "service_discovery": {
                "service_sources": 0,
                "degrade_metric_handles": 1,
                "degrade_metric_service_keys": ["default/deleted"],
                "degrade_metric_service_keys_truncated": false
            }}),
            &mut retained,
        );
        assert_eq!(
            retained.residue.get("gateway.service_discovery.degrade_metric_handles"),
            Some(&1)
        );
        let mut released = Snapshot::default();
        walk_json(
            "gateway",
            &json!({ "service_discovery": {
                "service_sources": 0,
                "degrade_metric_handles": 0,
                "degrade_metric_service_keys": [],
                "degrade_metric_service_keys_truncated": false
            }}),
            &mut released,
        );
        assert!(released.is_empty());
    }

    #[test]
    fn controller_configmap_ignores_require_exact_namespaced_keys() {
        let values = vec!["app/kube-root-ca.crt".to_string(), "test/kube-root-ca.crt".to_string()];
        let parsed = parse_controller_configmap_ignores(&values).unwrap();
        assert_eq!(parsed, BTreeSet::from_iter(values));

        for invalid in ["", "missing-name/", "/missing-namespace", "too/many/parts"] {
            assert!(parse_controller_configmap_ignores(&[invalid.to_string()]).is_err());
        }
        assert!(parse_controller_configmap_ignores(&["test/platform-ca".to_string()]).is_err());
    }

    #[test]
    fn conf_sync_secret_ignores_are_deduplicated_and_restricted() {
        let values = vec![
            "edgion-system/edgion-conf-sync-ca".to_string(),
            "edgion-system/edgion-conf-sync-ca".to_string(),
            "edgion-system/edgion-conf-sync-certs".to_string(),
        ];
        let parsed = parse_controller_conf_sync_secret_ignores(&values).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parse_controller_conf_sync_secret_ignores(&["test/unrelated".to_string()]).is_err());
    }

    #[test]
    fn conf_sync_secret_profile_requires_exact_ownership_type_and_keys() {
        let key = "edgion-system/edgion-conf-sync-ca";
        let valid = json!({
            "metadata": {
                "namespace": "edgion-system",
                "name": "edgion-conf-sync-ca",
                "labels": { "edgion.io/managed-by": "conf-sync-ca" }
            },
            "type": "Opaque",
            "data": { "ca.crt": "Y2E=", "ca.key": "a2V5" }
        });
        assert!(is_verified_conf_sync_secret(&valid, key));

        for invalid in [
            json!({
                "metadata": { "namespace": "edgion-system", "name": "edgion-conf-sync-ca" },
                "type": "Opaque",
                "data": { "ca.crt": "Y2E=", "ca.key": "a2V5" }
            }),
            json!({
                "metadata": {
                    "namespace": "edgion-system",
                    "name": "edgion-conf-sync-ca",
                    "labels": { "edgion.io/managed-by": "conf-sync-ca" }
                },
                "type": "kubernetes.io/tls",
                "data": { "ca.crt": "Y2E=", "ca.key": "a2V5" }
            }),
            json!({
                "metadata": {
                    "namespace": "edgion-system",
                    "name": "edgion-conf-sync-ca",
                    "labels": { "edgion.io/managed-by": "conf-sync-ca" }
                },
                "type": "Opaque",
                "data": { "ca.crt": "Y2E=", "ca.key": "a2V5", "extra": "bGVhaw==" }
            }),
            json!({
                "metadata": {
                    "namespace": "edgion-system",
                    "name": "edgion-conf-sync-ca",
                    "labels": { "edgion.io/managed-by": "conf-sync-ca" }
                },
                "type": "Opaque",
                "data": { "ca.crt": "Y2E=" }
            }),
            json!({
                "metadata": {
                    "namespace": "edgion-system",
                    "name": "edgion-conf-sync-ca",
                    "labels": { "edgion.io/managed-by": "conf-sync-ca" }
                },
                "type": "Opaque",
                "data": { "ca.crt": "" , "ca.key": "a2V5" }
            }),
        ] {
            assert!(!is_verified_conf_sync_secret(&invalid, key));
        }
    }

    #[test]
    fn runtime_secret_key_list_must_match_the_reported_count() {
        assert_eq!(
            parse_secret_store_keys(&json!({
                "secret_store": 2,
                "secret_store_keys": ["a/one", "b/two"]
            })),
            Some(BTreeSet::from(["a/one".to_string(), "b/two".to_string()]))
        );
        assert!(parse_secret_store_keys(&json!({
            "secret_store": 2,
            "secret_store_keys": ["a/one"]
        }))
        .is_none());
        assert!(parse_secret_store_keys(&json!({
            "secret_store": 2,
            "secret_store_keys": ["a/one", "a/one"]
        }))
        .is_none());
    }

    #[test]
    fn root_ca_configmap_profile_is_revalidated_after_wipe() {
        let key = "app/kube-root-ca.crt";
        let valid = json!({
            "metadata": {
                "namespace": "app",
                "name": "kube-root-ca.crt",
                "annotations": { "kubernetes.io/description": ROOT_CA_DESCRIPTION }
            },
            "data": { "ca.crt": "test-ca" }
        });
        assert!(is_verified_root_ca_configmap(&valid, key));

        for invalid in [
            json!({
                "metadata": { "namespace": "app", "name": "kube-root-ca.crt" },
                "data": { "ca.crt": "test-ca" }
            }),
            json!({
                "metadata": {
                    "namespace": "app",
                    "name": "kube-root-ca.crt",
                    "annotations": { "kubernetes.io/description": ROOT_CA_DESCRIPTION }
                },
                "data": { "ca.crt": "test-ca", "extra": "leak" }
            }),
            json!({
                "metadata": {
                    "namespace": "app",
                    "name": "kube-root-ca.crt",
                    "annotations": { "kubernetes.io/description": ROOT_CA_DESCRIPTION }
                },
                "data": { "ca.crt": "test-ca" },
                "binaryData": { "extra": "bGVhaw==" }
            }),
        ] {
            assert!(!is_verified_root_ca_configmap(&invalid, key));
        }
    }
}

const STAGED_BODY_CAP: usize = 8 * 1024 * 1024;
const STAGED_PAGE_SIZE: usize = 500;
const STAGED_MAX_PAGES: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
struct StagedKey {
    kind: ResourceKind,
    namespace: Option<String>,
    name: String,
    version: String,
    service_label: Option<String>,
}

impl StagedKey {
    fn identity(&self) -> String {
        self.namespace
            .as_ref()
            .map_or_else(|| self.name.clone(), |ns| format!("{ns}/{}", self.name))
    }
}

fn staged_tier(kind: ResourceKind) -> u8 {
    use ResourceKind::*;
    match kind {
        HTTPRoute
        | GRPCRoute
        | TCPRoute
        | UDPRoute
        | TLSRoute
        | EdgionPlugins
        | EdgionStreamPlugins
        | BackendTLSPolicy
        | EdgionBackendTrafficPolicy => 1,
        Gateway | GatewayClass | EdgionGatewayConfig | EdgionTls | EdgionAcme | EdgionConfigData | LinkSys
        | EdgionBackend | ReferenceGrant => 2,
        Service => 3,
        EndpointSlice => 4,
        Secret | ConfigMap => 5,
        Unspecified => 0,
    }
}

fn staged_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
}

fn staged_page(body: &Value, kind: ResourceKind) -> anyhow::Result<(Vec<StagedKey>, Option<String>)> {
    anyhow::ensure!(body["success"] == true, "metadata inventory API failed");
    let items = body["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("metadata inventory omitted data array"))?;
    anyhow::ensure!(
        body["count"].as_u64() == Some(items.len() as u64) && items.len() <= STAGED_PAGE_SIZE,
        "metadata inventory count or page size is invalid"
    );
    let info = ALL_RESOURCE_INFOS
        .iter()
        .find(|info| info.kind == kind)
        .ok_or_else(|| anyhow::anyhow!("metadata inventory kind is not canonical"))?;
    let mut keys = Vec::new();
    let mut seen = BTreeSet::new();
    for item in items {
        anyhow::ensure!(
            item["kind"] == kind.as_str() && item["apiVersion"] == "meta.k8s.io/v1",
            "metadata inventory resource type is invalid"
        );
        let metadata = &item["metadata"];
        let name = metadata["name"]
            .as_str()
            .filter(|v| staged_identifier(v))
            .ok_or_else(|| anyhow::anyhow!("metadata inventory resource name is invalid"))?;
        let namespace = match metadata.get("namespace") {
            None if info.cluster_scoped => None,
            Some(Value::String(value)) if !info.cluster_scoped && staged_identifier(value) => Some(value.clone()),
            _ => anyhow::bail!("metadata inventory resource scope is invalid"),
        };
        let version = metadata["resourceVersion"]
            .as_str()
            .filter(|v| !v.is_empty() && v.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| anyhow::anyhow!("metadata inventory resourceVersion is invalid"))?;
        let key = StagedKey {
            kind,
            namespace,
            name: name.into(),
            version: version.into(),
            service_label: metadata["labels"]["kubernetes.io/service-name"]
                .as_str()
                .map(str::to_owned),
        };
        anyhow::ensure!(
            seen.insert(key.identity()),
            "metadata inventory contains duplicate identities"
        );
        keys.push(key);
    }
    let next = match body.get("continue_token") {
        None => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => anyhow::bail!("metadata inventory continuation token is invalid"),
    };
    anyhow::ensure!(
        next.is_none() || !keys.is_empty(),
        "metadata inventory continuation made no progress"
    );
    Ok((keys, next))
}

struct StagedCleanup<'a> {
    client: &'a Client,
    args: &'a Args,
    deadline: Instant,
    configmaps: BTreeSet<String>,
    secrets: &'a BTreeSet<String>,
    leader: String,
    leader_uid: String,
    server_id: String,
}

impl StagedCleanup<'_> {
    fn remaining(&self) -> anyhow::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| anyhow::anyhow!("staged cleanup exceeded its total deadline"))
    }

    fn url(&self, base: &str, segments: &[&str]) -> anyhow::Result<reqwest::Url> {
        let mut url = reqwest::Url::parse(base)?;
        anyhow::ensure!(
            url.query().is_none() && url.fragment().is_none() && url.username().is_empty() && url.password().is_none(),
            "staged cleanup API address contains unsupported components"
        );
        let mut path = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid staged API address"))?;
        path.pop_if_empty();
        path.extend(segments.iter().copied());
        drop(path);
        Ok(url)
    }

    async fn json(&self, request: reqwest::RequestBuilder) -> anyhow::Result<Value> {
        let mut response = request
            .timeout(self.remaining()?.min(HTTP_REQUEST_TIMEOUT))
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            self.remaining()?;
            anyhow::ensure!(
                chunk.len() <= STAGED_BODY_CAP.saturating_sub(bytes.len()),
                "staged API response exceeded its body cap"
            );
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("staged API response is not valid JSON"))
    }

    fn object_url(&self, key: &StagedKey) -> anyhow::Result<reqwest::Url> {
        match &key.namespace {
            Some(ns) => self.url(
                &self.args.controller_addr,
                &["api", "v1", "namespaced", key.kind.as_str(), ns, &key.name],
            ),
            None => self.url(
                &self.args.controller_addr,
                &["api", "v1", "cluster", key.kind.as_str(), &key.name],
            ),
        }
    }

    async fn inventory(&self, kind: ResourceKind) -> anyhow::Result<Vec<StagedKey>> {
        let info = ALL_RESOURCE_INFOS
            .iter()
            .find(|info| info.kind == kind)
            .ok_or_else(|| anyhow::anyhow!("noncanonical inventory kind"))?;
        let scope = if info.cluster_scoped { "cluster" } else { "namespaced" };
        let url = self.url(&self.args.controller_addr, &["api", "v1", "keys", scope, kind.as_str()])?;
        let mut token: Option<String> = None;
        let mut tokens = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut keys = Vec::new();
        let mut restarts = 0;
        for _ in 0..STAGED_MAX_PAGES {
            let mut request = with_bearer(self.client.get(url.clone()), self.args.controller_token.as_deref())
                .query(&[("limit", STAGED_PAGE_SIZE.to_string())]);
            if let Some(value) = &token {
                request = request.query(&[("continue", value)]);
            }
            let body = match self.json(request).await {
                Ok(body) => body,
                Err(error)
                    if token.is_some()
                        && error.downcast_ref::<reqwest::Error>().and_then(|error| error.status())
                            == Some(reqwest::StatusCode::GONE) =>
                {
                    anyhow::ensure!(
                        restarts < 2,
                        "metadata inventory exhausted its stale pagination restarts"
                    );
                    restarts += 1;
                    keys.clear();
                    identities.clear();
                    tokens.clear();
                    token = None;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let (page, next) = staged_page(&body, kind)?;
            for key in page {
                anyhow::ensure!(
                    identities.insert(key.identity()),
                    "metadata inventory repeated an identity across pages"
                );
                keys.push(key);
            }
            match next {
                None => return Ok(keys),
                Some(next) => {
                    anyhow::ensure!(
                        tokens.insert(next.clone()),
                        "metadata inventory repeated a continuation token"
                    );
                    token = Some(next);
                }
            }
        }
        anyhow::bail!("metadata inventory exceeded its page limit")
    }

    async fn object(&self, key: &StagedKey) -> anyhow::Result<Value> {
        self.json(with_bearer(
            self.client.get(self.object_url(key)?),
            self.args.controller_token.as_deref(),
        ))
        .await
    }

    fn refreshed_key(&self, key: &StagedKey, object: &Value) -> anyhow::Result<StagedKey> {
        let metadata = &object["metadata"];
        let namespace_matches = match &key.namespace {
            Some(namespace) => metadata["namespace"].as_str() == Some(namespace.as_str()),
            None => metadata.get("namespace").is_none(),
        };
        anyhow::ensure!(
            object["kind"].as_str() == Some(key.kind.as_str())
                && metadata["name"].as_str() == Some(key.name.as_str())
                && namespace_matches,
            "fresh deletion observation has a different resource identity"
        );
        let uid = metadata["uid"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("fresh deletion observation omitted resource UID"))?;
        let version = metadata["resourceVersion"]
            .as_str()
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| anyhow::anyhow!("fresh deletion observation has an invalid resourceVersion"))?;
        if key.kind == ResourceKind::Service && key.identity() == self.leader {
            anyhow::ensure!(
                uid == self.leader_uid
                    && object["spec"]["selector"]
                        == serde_json::json!({"app":"edgion-controller", "edgion.io/leader":"true"}),
                "leader Service identity or selector changed before deletion"
            );
        }
        let mut refreshed = key.clone();
        refreshed.version = version.into();
        Ok(refreshed)
    }

    async fn delete(&self, key: &StagedKey) -> anyhow::Result<()> {
        // Earlier deletions requeue dependents and change their status RVs.
        // Read the current RV before the first mutation, retaining native CAS.
        let object = match self.object(key).await {
            Ok(object) => object,
            Err(error)
                if error.downcast_ref::<reqwest::Error>().and_then(|error| error.status())
                    == Some(reqwest::StatusCode::NOT_FOUND) =>
            {
                eprintln!(
                    "[INFO] staged absent: kind={} key={}",
                    key.kind.as_str(),
                    key.identity()
                );
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let key = self.refreshed_key(key, &object)?;
        let url = self.object_url(&key)?;
        // One request only: timeouts and version conflicts must never replay a mutation.
        let response = with_bearer(self.client.delete(url.clone()), self.args.controller_token.as_deref())
            .header(reqwest::header::IF_MATCH, format!("\"{}\"", key.version))
            .timeout(self.remaining()?.min(HTTP_REQUEST_TIMEOUT))
            .send()
            .await?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            let observed = with_bearer(self.client.get(url), self.args.controller_token.as_deref())
                .timeout(self.remaining()?.min(HTTP_REQUEST_TIMEOUT))
                .send()
                .await?;
            anyhow::ensure!(
                observed.status() == reqwest::StatusCode::NOT_FOUND,
                "delete 404 was not confirmed by a fresh absence observation"
            );
        } else {
            anyhow::ensure!(
                status == reqwest::StatusCode::NO_CONTENT,
                "versioned deletion failed for {} {}: status={status}",
                key.kind.as_str(),
                key.identity()
            );
        }
        eprintln!(
            "[INFO] staged delete: kind={} key={} resourceVersion={}",
            key.kind.as_str(),
            key.identity(),
            key.version
        );
        Ok(())
    }

    fn baseline(&self, key: &StagedKey) -> bool {
        (key.kind == ResourceKind::Secret && self.secrets.contains(&key.identity()))
            || (key.kind == ResourceKind::ConfigMap && self.configmaps.contains(&key.identity()))
    }

    fn leader_slice(&self, key: &StagedKey) -> bool {
        key.kind == ResourceKind::EndpointSlice
            && key.namespace.as_deref() == self.leader.split_once('/').map(|pair| pair.0)
            && key.service_label.as_deref() == self.leader.split_once('/').map(|pair| pair.1)
    }

    async fn verify_slice(&self, key: &StagedKey) -> anyhow::Result<()> {
        let object = self.object(key).await?;
        let name = self.leader.split_once('/').unwrap().1;
        anyhow::ensure!(
            object["metadata"]["ownerReferences"]
                .as_array()
                .is_some_and(|owners| owners.iter().any(|owner| owner["kind"] == "Service"
                    && owner["name"] == name
                    && owner["uid"] == self.leader_uid
                    && owner["controller"] == true)),
            "leader EndpointSlice does not belong to the verified live Service"
        );
        Ok(())
    }

    async fn verify_baseline(&self, key: &StagedKey) -> anyhow::Result<()> {
        let object = self.object(key).await?;
        let valid = match key.kind {
            ResourceKind::Secret => is_verified_conf_sync_secret(&object, &key.identity()),
            ResourceKind::ConfigMap => is_verified_root_ca_configmap(&object, &key.identity()),
            _ => false,
        };
        anyhow::ensure!(valid, "retained baseline failed its existing fixture profile");
        Ok(())
    }

    async fn ordinary_converged(&self) -> anyhow::Result<bool> {
        let mut retained = BTreeMap::new();
        for info in ALL_RESOURCE_INFOS {
            let keys = self.inventory(info.kind).await?;
            let mut expected = BTreeSet::new();
            for key in &keys {
                if self.baseline(key) {
                    self.verify_baseline(key).await?;
                } else if key.kind == ResourceKind::Service && key.identity() == self.leader {
                } else if self.leader_slice(key) {
                    self.verify_slice(key).await?;
                } else {
                    return Ok(false);
                }
                expected.insert(key.identity());
            }
            if info.kind == ResourceKind::Secret && expected != *self.secrets {
                return Ok(false);
            }
            if info.kind == ResourceKind::ConfigMap && expected != self.configmaps {
                return Ok(false);
            }
            if info.kind == ResourceKind::Service && expected != BTreeSet::from([self.leader.clone()]) {
                return Ok(false);
            }
            if info.kind == ResourceKind::EndpointSlice && expected.is_empty() {
                return Ok(false);
            }
            retained.insert(info.kind.as_str(), expected);
        }
        for info in ALL_RESOURCE_INFOS.iter().filter(|info| {
            info.in_registry && !edgion_resources::resource_defs::DEFAULT_NO_SYNC_KINDS.contains(&info.kind.as_str())
        }) {
            let url = self.url(&self.args.gateway_addr, &["configclient", info.kind.as_str(), "list"])?;
            let body = self
                .json(with_bearer(self.client.get(url), self.args.gateway_token.as_deref()))
                .await?;
            anyhow::ensure!(body["success"] == true, "Gateway cache inventory API failed");
            let items = body["data"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Gateway cache inventory omitted array"))?;
            let mut actual = BTreeSet::new();
            for item in items {
                let metadata = &item["metadata"];
                let name = metadata["name"].as_str().filter(|name| staged_identifier(name));
                let namespace = metadata.get("namespace").and_then(Value::as_str);
                anyhow::ensure!(
                    name.is_some()
                        && if info.cluster_scoped {
                            metadata.get("namespace").is_none()
                        } else {
                            namespace.is_some_and(staged_identifier)
                        },
                    "Gateway cache inventory identity is invalid"
                );
                let key =
                    resource_key(item).ok_or_else(|| anyhow::anyhow!("Gateway cache inventory identity is invalid"))?;
                anyhow::ensure!(actual.insert(key), "Gateway cache inventory duplicated identity");
            }
            if Some(&actual) != retained.get(info.kind.as_str()) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn server_identity(&self) -> anyhow::Result<String> {
        let url = self.url(&self.args.controller_addr, &["api", "v1", "server-info"])?;
        let body = self
            .json(with_bearer(self.client.get(url), self.args.controller_token.as_deref()))
            .await?;
        anyhow::ensure!(
            body["success"] == true && body["data"]["ready"] == true,
            "Controller is not ready for staged cleanup"
        );
        body["data"]["server_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("Controller server identity is missing"))
    }
}

async fn staged_wipe(
    client: &Client,
    args: &Args,
    configmaps: &BTreeSet<String>,
    secrets: &BTreeSet<String>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.skip.is_none() && args.mode == "fail",
        "staged cleanup requires both validators in fail mode"
    );
    let leader = args
        .k8s_leader_service
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("staged cleanup requires an exact leader Service"))?;
    let (namespace, name) = leader
        .split_once('/')
        .filter(|(ns, name)| staged_identifier(ns) && staged_identifier(name))
        .ok_or_else(|| anyhow::anyhow!("invalid staged leader Service identity"))?;
    anyhow::ensure!(
        name == "edgion-controller-leader",
        "staged cleanup only accepts the canonical conf-sync leader Service"
    );
    anyhow::ensure!(
        secrets
            == &BTreeSet::from([
                format!("{namespace}/edgion-conf-sync-ca"),
                format!("{namespace}/edgion-conf-sync-certs")
            ]),
        "staged cleanup requires exactly the two verified conf-sync Secrets"
    );
    let mut cleanup = StagedCleanup {
        client,
        args,
        deadline: Instant::now() + Duration::from_secs(args.wipe_timeout_secs),
        configmaps: configmaps.clone(),
        secrets,
        leader: leader.into(),
        leader_uid: String::new(),
        server_id: String::new(),
    };
    cleanup.server_id = cleanup.server_identity().await?;
    let leader_key = cleanup
        .inventory(ResourceKind::Service)
        .await?
        .into_iter()
        .find(|key| key.identity() == leader)
        .ok_or_else(|| anyhow::anyhow!("leader Service is absent from Controller inventory"))?;
    let service = cleanup.object(&leader_key).await?;
    anyhow::ensure!(
        service["spec"]["selector"] == serde_json::json!({"app":"edgion-controller", "edgion.io/leader":"true"}),
        "leader Service selector differs from the canonical fixture"
    );
    cleanup.leader_uid = service["metadata"]["uid"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("leader Service UID is missing"))?;
    // The gate also verifies roots outside the Controller's watch namespaces.
    // Retain only the exact already-visible intersection, never require unseen keys.
    let visible_roots = cleanup.inventory(ResourceKind::ConfigMap).await?;
    cleanup.configmaps = visible_roots
        .iter()
        .filter(|key| configmaps.contains(&key.identity()))
        .map(StagedKey::identity)
        .collect();
    for key in visible_roots
        .iter()
        .filter(|key| cleanup.configmaps.contains(&key.identity()))
    {
        cleanup.verify_baseline(key).await?;
    }
    let secret_keys = cleanup.inventory(ResourceKind::Secret).await?;
    let retained_secrets = secret_keys
        .iter()
        .filter(|key| secrets.contains(&key.identity()))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        retained_secrets.len() == 2,
        "verified conf-sync Secrets are not both Controller-visible"
    );
    for key in retained_secrets {
        cleanup.verify_baseline(key).await?;
    }
    let leader_slices = cleanup.inventory(ResourceKind::EndpointSlice).await?;
    let leader_slices = leader_slices
        .iter()
        .filter(|key| cleanup.leader_slice(key))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !leader_slices.is_empty(),
        "leader Service has no Controller-visible EndpointSlice"
    );
    for key in leader_slices {
        cleanup.verify_slice(key).await?;
    }
    // Sequential deletion keeps the mutation journal and ambiguous failures bounded.
    for tier in 1..=5 {
        let mut inventory = Vec::new();
        for info in ALL_RESOURCE_INFOS.iter().filter(|info| staged_tier(info.kind) == tier) {
            inventory.extend(cleanup.inventory(info.kind).await?);
        }
        for key in &inventory {
            eprintln!(
                "[INFO] staged inventory: tier={tier} kind={} key={} resourceVersion={}",
                key.kind.as_str(),
                key.identity(),
                key.version
            );
        }
        for key in inventory {
            if cleanup.baseline(&key) {
                cleanup.verify_baseline(&key).await?;
            } else if key.kind == ResourceKind::Service && key.identity() == leader {
            } else if cleanup.leader_slice(&key) {
                cleanup.verify_slice(&key).await?;
            } else {
                cleanup.delete(&key).await?;
            }
        }
    }
    loop {
        anyhow::ensure!(
            cleanup.server_identity().await? == cleanup.server_id,
            "Controller restarted during staged cleanup"
        );
        if cleanup.ordinary_converged().await? {
            break;
        }
        tokio::time::sleep(cleanup.remaining()?.min(Duration::from_millis(args.poll_interval_ms))).await;
    }
    // Re-inventory immediately before the last mutation; never delete leader slices.
    let remaining = cleanup.inventory(ResourceKind::Service).await?;
    anyhow::ensure!(
        remaining.len() == 1 && remaining[0].identity() == leader,
        "ordinary Services did not remain empty"
    );
    cleanup.delete(&remaining[0]).await?;
    eprintln!("[INFO] staged cleanup complete; leader EndpointSlices must disappear through real Kubernetes GC");
    Ok(())
}

#[cfg(test)]
mod staged_cleanup_tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        matchers::{any, header, method, path, query_param},
        Mock, MockServer, Request, Respond, ResponseTemplate,
    };

    fn key(kind: ResourceKind, namespace: Option<&str>, name: &str) -> Value {
        let mut value = json!({"kind":kind.as_str(), "apiVersion":"meta.k8s.io/v1", "metadata":{"name":name,"resourceVersion":"123","uid":"test-uid"}});
        if let Some(ns) = namespace {
            value["metadata"]["namespace"] = json!(ns);
        }
        value
    }

    fn page(items: Vec<Value>) -> Value {
        json!({"success":true,"count":items.len(),"data":items})
    }

    fn args(server: &MockServer) -> Args {
        Args::try_parse_from([
            "cache_residue",
            "--controller-addr",
            &server.uri(),
            "--gateway-addr",
            &server.uri(),
            "--controller-token",
            "test-token",
            "--gateway-token",
            "test-token",
            "--mode",
            "fail",
            "--k8s-staged-wipe",
            "--k8s-leader-service",
            "system/edgion-controller-leader",
        ])
        .unwrap()
    }

    fn context<'a>(client: &'a Client, args: &'a Args, secrets: &'a BTreeSet<String>) -> StagedCleanup<'a> {
        StagedCleanup {
            client,
            args,
            deadline: Instant::now() + Duration::from_secs(2),
            configmaps: BTreeSet::new(),
            secrets,
            leader: "system/edgion-controller-leader".into(),
            leader_uid: "leader-uid".into(),
            server_id: "s1".into(),
        }
    }

    #[test]
    fn metadata_contract_rejects_foreign_types_missing_versions_scopes_and_duplicates() {
        let valid = key(ResourceKind::Service, Some("test"), "backend");
        assert!(staged_page(&page(vec![valid.clone()]), ResourceKind::Service).is_ok());
        for (pointer, invalid) in [
            ("/kind", json!("Secret")),
            ("/apiVersion", json!("v1")),
            ("/metadata/resourceVersion", json!("")),
            ("/metadata/resourceVersion", json!("*")),
            ("/metadata/namespace", Value::Null),
            ("/metadata/name", json!("bad/name")),
        ] {
            let mut item = valid.clone();
            *item.pointer_mut(pointer).unwrap() = invalid;
            assert!(staged_page(&page(vec![item]), ResourceKind::Service).is_err());
        }
        assert!(staged_page(&page(vec![valid.clone(), valid]), ResourceKind::Service).is_err());
        let mut invalid = page(vec![]);
        invalid["count"] = json!(1);
        assert!(staged_page(&invalid, ResourceKind::Service).is_err());
        let cluster = key(ResourceKind::GatewayClass, None, "class");
        assert!(staged_page(&page(vec![cluster.clone()]), ResourceKind::GatewayClass).is_ok());
        let mut invalid = cluster;
        invalid["metadata"]["namespace"] = json!("test");
        assert!(staged_page(&page(vec![invalid]), ResourceKind::GatewayClass).is_err());
        assert!(ALL_RESOURCE_INFOS.iter().all(|info| staged_tier(info.kind) > 0));
        assert_eq!(
            ALL_RESOURCE_INFOS
                .iter()
                .filter(|info| staged_tier(info.kind) == 1)
                .count(),
            9
        );
    }

    #[tokio::test]
    async fn metadata_pagination_validates_each_page_and_rejects_repeated_tokens_or_keys() {
        for duplicate in [false, true] {
            let server = MockServer::start().await;
            let mut first = page(vec![key(ResourceKind::Service, Some("test"), "one")]);
            first["continue_token"] = json!("next");
            Mock::given(method("GET"))
                .and(path("/api/v1/keys/namespaced/Service"))
                .and(query_param("limit", "500"))
                .respond_with(ResponseTemplate::new(200).set_body_json(first))
                .mount(&server)
                .await;
            let mut second = page(vec![key(
                ResourceKind::Service,
                Some("test"),
                if duplicate { "one" } else { "two" },
            )]);
            if !duplicate {
                second["continue_token"] = json!("next");
            }
            Mock::given(method("GET"))
                .and(query_param("continue", "next"))
                .respond_with(ResponseTemplate::new(200).set_body_json(second))
                .with_priority(1)
                .expect(1)
                .mount(&server)
                .await;
            let args = args(&server);
            let client = Client::new();
            let secrets = BTreeSet::new();
            assert!(context(&client, &args, &secrets)
                .inventory(ResourceKind::Service)
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn metadata_expired_cursor_discards_the_entire_previous_snapshot() {
        let server = MockServer::start().await;
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = requests.clone();
        Mock::given(method("GET"))
            .and(path("/api/v1/keys/namespaced/Service"))
            .and(query_param("limit", "500"))
            .respond_with(move |request: &Request| {
                let index = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let continued = request
                    .url
                    .query_pairs()
                    .any(|(name, value)| name == "continue" && value == "next");
                assert_eq!(continued, index % 2 == 1);
                if index == 1 {
                    return ResponseTemplate::new(410);
                }
                let mut shared = key(ResourceKind::Service, Some("test"), "shared");
                shared["metadata"]["resourceVersion"] = json!(if index == 0 { "123" } else { "124" });
                let mut body = match index {
                    0 => page(vec![key(ResourceKind::Service, Some("test"), "obsolete"), shared]),
                    2 => page(vec![shared]),
                    3 => page(vec![key(ResourceKind::Service, Some("test"), "current")]),
                    _ => panic!("unexpected inventory request"),
                };
                if index != 3 {
                    body["continue_token"] = json!("next");
                }
                ResponseTemplate::new(200).set_body_json(body)
            })
            .expect(4)
            .mount(&server)
            .await;
        let args = args(&server);
        let client = Client::new();
        let secrets = BTreeSet::new();
        let keys = context(&client, &args, &secrets)
            .inventory(ResourceKind::Service)
            .await
            .unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!((&*keys[0].name, &*keys[0].version), ("shared", "124"));
        assert_eq!(keys[1].name, "current");
        assert_eq!(server.received_requests().await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn metadata_cursor_restarts_are_bounded_and_only_apply_to_continued_gone() {
        for (status, continued, expected_requests) in [(410, true, 6), (410, false, 1), (500, true, 2)] {
            let server = MockServer::start().await;
            let mut first = page(vec![key(ResourceKind::Service, Some("test"), "one")]);
            first["continue_token"] = json!("next");
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(first))
                .mount(&server)
                .await;
            let mut failing = Mock::given(method("GET"));
            if continued {
                failing = failing.and(query_param("continue", "next"));
            }
            failing
                .respond_with(ResponseTemplate::new(status))
                .with_priority(1)
                .mount(&server)
                .await;
            let args = args(&server);
            let client = Client::new();
            let secrets = BTreeSet::new();
            let error = context(&client, &args, &secrets)
                .inventory(ResourceKind::Service)
                .await
                .unwrap_err();
            if continued && status == 410 {
                assert!(error.to_string().contains("exhausted"));
            }
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), expected_requests);
            assert!(requests.iter().all(|request| request.method == "GET"));
        }
    }

    #[tokio::test]
    async fn guarded_delete_never_replays_conflicts_timeouts_or_unconfirmed_gc() {
        for status in [204, 409, 500, 404] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/v1/namespaced/Service/test/backend"))
                .respond_with(ResponseTemplate::new(200).set_body_json(key(
                    ResourceKind::Service,
                    Some("test"),
                    "backend",
                )))
                .expect(if status == 404 { 2 } else { 1 })
                .mount(&server)
                .await;
            Mock::given(method("DELETE"))
                .and(path("/api/v1/namespaced/Service/test/backend"))
                .and(header("if-match", "\"123\""))
                .and(header("authorization", "Bearer test-token"))
                .respond_with(ResponseTemplate::new(status))
                .expect(1)
                .mount(&server)
                .await;
            let args = args(&server);
            let client = Client::new();
            let secrets = BTreeSet::new();
            let keys = staged_page(
                &page(vec![key(ResourceKind::Service, Some("test"), "backend")]),
                ResourceKind::Service,
            )
            .unwrap()
            .0;
            assert_eq!(
                context(&client, &args, &secrets).delete(&keys[0]).await.is_ok(),
                status == 204
            );
        }
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(key(ResourceKind::Service, Some("test"), "backend")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_millis(100)))
            .expect(1)
            .mount(&server)
            .await;
        let args = args(&server);
        let client = Client::new();
        let secrets = BTreeSet::new();
        let mut cleanup = context(&client, &args, &secrets);
        cleanup.deadline = Instant::now() + Duration::from_millis(20);
        let keys = staged_page(
            &page(vec![key(ResourceKind::Service, Some("test"), "backend")]),
            ResourceKind::Service,
        )
        .unwrap()
        .0;
        assert!(cleanup.delete(&keys[0]).await.is_err());
    }

    #[tokio::test]
    async fn deletion_uses_current_rv_once_instead_of_the_stale_tier_snapshot() {
        let server = MockServer::start().await;
        let original = key(ResourceKind::HTTPRoute, Some("test"), "parent");
        let mut current = original.clone();
        current["metadata"]["resourceVersion"] = json!("124");
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaced/HTTPRoute/test/parent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(current))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/namespaced/HTTPRoute/test/parent"))
            .and(header("if-match", "\"124\""))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let args = args(&server);
        let client = Client::new();
        let secrets = BTreeSet::new();
        let selected = staged_page(&page(vec![original]), ResourceKind::HTTPRoute).unwrap().0;
        context(&client, &args, &secrets).delete(&selected[0]).await.unwrap();
        assert_eq!(selected[0].version, "123");
    }

    #[tokio::test]
    async fn malformed_current_identity_uid_or_rv_never_mutates() {
        for (pointer, invalid) in [
            ("/kind", json!("Secret")),
            ("/metadata/name", json!("different")),
            ("/metadata/namespace", json!("different")),
            ("/metadata/uid", json!("")),
            ("/metadata/uid", Value::Null),
            ("/metadata/resourceVersion", json!("")),
            ("/metadata/resourceVersion", json!("124\"")),
        ] {
            let server = MockServer::start().await;
            let original = key(ResourceKind::Service, Some("test"), "backend");
            let mut current = original.clone();
            *current.pointer_mut(pointer).unwrap() = invalid;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(current))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("DELETE"))
                .respond_with(ResponseTemplate::new(204))
                .expect(0)
                .mount(&server)
                .await;
            let args = args(&server);
            let client = Client::new();
            let secrets = BTreeSet::new();
            let selected = staged_page(&page(vec![original]), ResourceKind::Service).unwrap().0;
            assert!(context(&client, &args, &secrets).delete(&selected[0]).await.is_err());
        }
    }

    #[tokio::test]
    async fn initial_absence_requires_no_delete_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;
        let args = args(&server);
        let client = Client::new();
        let secrets = BTreeSet::new();
        let selected = staged_page(
            &page(vec![key(ResourceKind::Service, Some("test"), "backend")]),
            ResourceKind::Service,
        )
        .unwrap()
        .0;
        context(&client, &args, &secrets).delete(&selected[0]).await.unwrap();
    }

    #[tokio::test]
    async fn replacement_leader_or_changed_selector_is_preserved_without_delete() {
        for change_uid in [false, true] {
            let server = MockServer::start().await;
            let original = key(ResourceKind::Service, Some("system"), "edgion-controller-leader");
            let mut current = original.clone();
            current["metadata"]["uid"] = json!(if change_uid { "replacement-uid" } else { "leader-uid" });
            current["spec"]["selector"] =
                json!({"app":"edgion-controller", "edgion.io/leader":if change_uid { "true" } else { "false" }});
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(current))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("DELETE"))
                .respond_with(ResponseTemplate::new(204))
                .expect(0)
                .mount(&server)
                .await;
            let args = args(&server);
            let client = Client::new();
            let secrets = BTreeSet::new();
            let selected = staged_page(&page(vec![original]), ResourceKind::Service).unwrap().0;
            assert!(context(&client, &args, &secrets).delete(&selected[0]).await.is_err());
        }
    }

    #[derive(Clone)]
    struct Fixture(std::sync::Arc<parking_lot::Mutex<BTreeMap<(String, String), Value>>>);
    impl Respond for Fixture {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let segments = request.url.path().trim_matches('/').split('/').collect::<Vec<_>>();
            let mut objects = self.0.lock();
            if request.url.path() == "/api/v1/server-info" {
                return ResponseTemplate::new(200)
                    .set_body_json(json!({"success":true,"data":{"ready":true,"server_id":"s1"}}));
            }
            if segments.get(2) == Some(&"keys") {
                let kind = segments[4];
                let items = objects
                    .iter()
                    .filter(|((k, _), _)| k == kind)
                    .map(|(_, object)| json!({"kind":kind,"apiVersion":"meta.k8s.io/v1","metadata":object["metadata"]}))
                    .collect();
                return ResponseTemplate::new(200).set_body_json(page(items));
            }
            if segments.first() == Some(&"configclient") {
                let kind = segments[1];
                let items = objects
                    .iter()
                    .filter(|((k, _), _)| k == kind)
                    .map(|(_, v)| v.clone())
                    .collect::<Vec<_>>();
                return ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":items}));
            }
            let kind = segments[3];
            let identity = if segments[2] == "namespaced" {
                format!("{}/{}", segments[4], segments[5])
            } else {
                segments[4].into()
            };
            let tuple = (kind.into(), identity.clone());
            if request.method.as_str() == "DELETE" {
                if objects.remove(&tuple).is_none() {
                    return ResponseTemplate::new(404);
                }
                if kind == "Service" {
                    objects.retain(|(k, _), object| {
                        !(k == "EndpointSlice"
                            && object["metadata"]["labels"]["kubernetes.io/service-name"] == segments[5])
                    });
                }
                return ResponseTemplate::new(204);
            }
            objects.get(&tuple).map_or_else(
                || ResponseTemplate::new(404),
                |object| ResponseTemplate::new(200).set_body_json(object),
            )
        }
    }

    #[tokio::test]
    async fn real_service_gc_order_retains_only_visible_baselines_and_deletes_leader_last() {
        let server = MockServer::start().await;
        let mut objects = BTreeMap::new();
        for (kind, name, ns) in [
            ("Service", "edgion-controller-leader", "system"),
            ("Service", "ordinary", "test"),
            ("EndpointSlice", "leader-slice", "system"),
            ("EndpointSlice", "ordinary-slice", "test"),
            ("HTTPRoute", "consumer", "test"),
            ("Secret", "edgion-conf-sync-ca", "system"),
            ("Secret", "edgion-conf-sync-certs", "system"),
            ("Secret", "ordinary-secret", "test"),
            ("ConfigMap", "kube-root-ca.crt", "test"),
        ] {
            let mut object = json!({"kind":kind,"metadata":{"namespace":ns,"name":name,"resourceVersion":"123","uid":format!("{ns}-{name}-uid")}});
            match (kind, name) {
                ("Service", "edgion-controller-leader") => {
                    object["metadata"]["uid"] = json!("leader-uid");
                    object["spec"] = json!({"selector":{"app":"edgion-controller","edgion.io/leader":"true"}});
                }
                ("EndpointSlice", _) => {
                    let service = if ns == "system" {
                        "edgion-controller-leader"
                    } else {
                        "ordinary"
                    };
                    object["metadata"]["labels"] = json!({"kubernetes.io/service-name":service});
                    let owner_uid = if ns == "system" {
                        "leader-uid"
                    } else {
                        "test-ordinary-uid"
                    };
                    object["metadata"]["ownerReferences"] =
                        json!([{"kind":"Service","name":service,"uid":owner_uid,"controller":true}]);
                }
                ("Secret", "edgion-conf-sync-ca" | "edgion-conf-sync-certs") => {
                    object["metadata"]["labels"] = json!({"edgion.io/managed-by":"conf-sync-ca"});
                    object["type"] = json!("Opaque");
                    object["data"] = if name == "edgion-conf-sync-ca" {
                        json!({"ca.crt":"test","ca.key":"test"})
                    } else {
                        json!({"ca.crt":"test","client.crt":"test","client.key":"test"})
                    };
                }
                ("ConfigMap", _) => {
                    object["metadata"]["annotations"] = json!({"kubernetes.io/description":ROOT_CA_DESCRIPTION});
                    object["data"] = json!({"ca.crt":"test"});
                }
                _ => {}
            }
            objects.insert((kind.into(), format!("{ns}/{name}")), object);
        }
        let fixture = Fixture(std::sync::Arc::new(parking_lot::Mutex::new(objects)));
        Mock::given(any()).respond_with(fixture.clone()).mount(&server).await;
        let args = args(&server);
        let client = Client::new();
        let roots = BTreeSet::from(["test/kube-root-ca.crt".into(), "unwatched/kube-root-ca.crt".into()]);
        let secrets = BTreeSet::from([
            "system/edgion-conf-sync-ca".into(),
            "system/edgion-conf-sync-certs".into(),
        ]);
        staged_wipe(&client, &args, &roots, &secrets).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let deletes = requests
            .iter()
            .filter(|request| request.method.as_str() == "DELETE")
            .collect::<Vec<_>>();
        assert!(deletes.first().unwrap().url.path().contains("HTTPRoute"));
        assert!(deletes
            .last()
            .unwrap()
            .url
            .path()
            .ends_with("/Service/system/edgion-controller-leader"));
        assert!(!deletes
            .iter()
            .any(|request| request.url.path().contains("EndpointSlice")));
        let remaining = fixture.0.lock();
        assert_eq!(remaining.len(), 3);
        assert!(remaining
            .keys()
            .all(|(kind, _)| kind == "Secret" || kind == "ConfigMap"));
    }
}
