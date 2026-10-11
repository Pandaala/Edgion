//! Cache Residue Validator
//!
//! Asserts that all in-memory caches in controller and gateway reach zero
//! after `wipe-all`. Used as the final step of `run_integration.sh` to
//! detect leaks across all integration scenarios.
//!
//! See spec: docs/superpowers/specs/2026-04-26-cache-residue-detection-design.md

use clap::Parser;
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
                    "wasm_shared_data_metrics": {
                        "count": 1,
                        "keys": ["file:test-module.wasm"]
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

        assert_eq!(snap.residue.get("gateway.wasm_shared_data_metrics.count"), Some(&1));
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
