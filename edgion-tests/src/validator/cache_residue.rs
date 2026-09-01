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
const WIPE_ALL_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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

    #[arg(long, default_value = "30")]
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
    if !args.no_wipe && args.skip.as_deref() != Some("controller") {
        if let Err(e) = wipe_all(&client, &args.controller_addr, args.controller_token.as_deref()).await {
            eprintln!("[ERROR] wipe-all failed: {}", e);
            process::exit(2);
        }
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

async fn wipe_all(client: &Client, controller: &str, token: Option<&str>) -> reqwest::Result<()> {
    let url = format!("{}/api/v1/admin/wipe-all", controller);
    // A Kubernetes wipe deletes hundreds of objects through the API server and
    // can legitimately exceed the short timeout used by read-only polling.
    // Keep one extended request instead of retrying an idempotent-but-still-
    // running destructive operation after an ambiguous transport timeout.
    let resp = wipe_all_request(client, &url, token).send().await?.error_for_status()?;
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

fn wipe_all_request(client: &Client, url: &str, token: Option<&str>) -> reqwest::RequestBuilder {
    with_bearer(client.post(url), token).timeout(WIPE_ALL_REQUEST_TIMEOUT)
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
) -> reqwest::Result<Snapshot> {
    let mut snap = Snapshot::default();

    if args.skip.as_deref() != Some("controller") {
        let url = format!("{}/api/v1/debug/store-stats", args.controller_addr);
        let body: Value = with_bearer(client.get(&url), args.controller_token.as_deref())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let runtime_secret_keys = body.get("data").and_then(parse_secret_store_keys);
        if let Some(data) = body.get("data") {
            walk_json("controller", data, &mut snap);
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
        if let Some(data) = body.get("data") {
            walk_json("gateway", data, &mut snap);
        }
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
                "data": { "service_store": { "count": 1 } }
            })))
            .mount(&gw)
            .await;

        let args = Args {
            controller_addr: ctrl.uri(),
            gateway_addr: gw.uri(),
            controller_token: None,
            gateway_token: None,
            no_wipe: true,
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
        assert_eq!(snap.residue.get("gateway.service_store.count"), Some(&1));
        assert!(!snap
            .residue
            .contains_key("controller.event_store.next_resource_version"));
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
    fn wipe_all_overrides_the_short_polling_request_timeout() {
        let client = Client::builder().timeout(HTTP_REQUEST_TIMEOUT).build().unwrap();
        let request = wipe_all_request(&client, "http://127.0.0.1/wipe", None)
            .build()
            .unwrap();

        assert_eq!(request.timeout(), Some(&WIPE_ALL_REQUEST_TIMEOUT));
    }

    #[test]
    fn ignore_path_filters_event_store() {
        let mut snap = Snapshot::default();
        let v = json!({ "event_store": { "next_resource_version": 142 } });
        walk_json("controller", &v, &mut snap);
        assert!(snap.is_empty(), "ignored path must not appear");
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
        let v = json!({ "service_store": { "count": 0 } });
        walk_json("gateway", &v, &mut snap);
        assert!(snap.is_empty());
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
