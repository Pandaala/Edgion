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
use std::collections::BTreeMap;
use std::process;
use std::time::{Duration, Instant};

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
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
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
        match collect(&client, &args).await {
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
    let resp = with_bearer(client.post(&url), token).send().await?.error_for_status()?;
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

async fn clear_access_logs(client: &Client, gateway: &str, token: Option<&str>) -> reqwest::Result<()> {
    let url = format!("{}/api/v1/testing/access-logs", gateway);
    with_bearer(client.delete(&url), token)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn collect(client: &Client, args: &Args) -> reqwest::Result<Snapshot> {
    let mut snap = Snapshot::default();

    if args.skip.as_deref() != Some("controller") {
        let url = format!("{}/api/v1/debug/store-stats", args.controller_addr);
        let body: Value = with_bearer(client.get(&url), args.controller_token.as_deref())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(data) = body.get("data") {
            walk_json("controller", data, &mut snap);
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
        if let Some(samples) = fetch_samples(client, args, &path).await {
            snap.samples.insert(path, samples);
        }
    }

    Ok(snap)
}

async fn fetch_samples(client: &Client, args: &Args, path: &str) -> Option<Vec<String>> {
    if let Some(rest) = path.strip_prefix("controller.conf_center_storage.") {
        let url = format!("{}/api/v1/namespaced/{}", args.controller_addr, rest);
        let body: Value = with_bearer(client.get(&url), args.controller_token.as_deref())
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let items = body.pointer("/data")?.as_array()?;
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
        };
        let client = Client::new();
        let snap = collect(&client, &args).await.unwrap();
        assert_eq!(snap.residue.get("controller.secret_store"), Some(&2));
        assert_eq!(snap.residue.get("gateway.service_store.count"), Some(&1));
        assert!(!snap
            .residue
            .contains_key("controller.event_store.next_resource_version"));
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use serde_json::json;

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
}
