use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::time::{Duration, Instant};

const NS: &str = "edgion-default";
const KIND: &str = "EdgionBackendTrafficPolicy";

pub struct EdgionBackendTrafficPolicyTestSuite;

// ---------------------------------------------------------------------------
// Gateway admin helpers (port 12001, EDGION_GATEWAY_ADMIN_TOKEN bearer).
// ---------------------------------------------------------------------------

fn gw_admin_base() -> String {
    std::env::var("EDGION_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:12001".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn gw_admin_token() -> Option<String> {
    std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
}

fn admin_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .expect("build admin client")
}

async fn gw_get_json(client: &reqwest::Client, path: &str) -> Result<Value, String> {
    let url = format!("{}{}", gw_admin_base(), path);
    let mut req = client.get(&url);
    if let Some(t) = gw_admin_token() {
        req = req.header("Authorization", format!("Bearer {}", t));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("gateway admin GET {} failed: {}", path, e))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("gateway admin GET {} -> {}: {}", path, status, body));
    }
    serde_json::from_str(&body).map_err(|e| format!("gateway admin GET {} bad JSON: {}", path, e))
}

/// service_key -> algorithm debug string, from /api/v1/debug/store-stats.
async fn lb_policy_for(client: &reqwest::Client, service_key: &str) -> Result<Option<String>, String> {
    let v = gw_get_json(client, "/api/v1/debug/store-stats").await?;
    let map = v
        .get("data")
        .and_then(|d| d.get("lb_policies"))
        .and_then(|x| x.as_object())
        .ok_or_else(|| "store-stats response missing data.lb_policies".to_string())?;
    Ok(map.get(service_key).and_then(|x| x.as_str()).map(|s| s.to_string()))
}

/// Poll the resolved LB algorithm for a service key until `pred` accepts the
/// observed value (or `None` when absent). Returns (matched, last_observed).
async fn poll_lb<F>(client: &reqwest::Client, service_key: &str, pred: F, tries: usize) -> (bool, Option<String>)
where
    F: Fn(Option<&str>) -> bool,
{
    let mut last = None;
    for _ in 0..tries {
        if let Ok(v) = lb_policy_for(client, service_key).await {
            if pred(v.as_deref()) {
                return (true, v);
            }
            last = v;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    (false, last)
}

const OBSERVATION_BODY_LIMIT: usize = 128 * 1024;
const OBSERVATION_CANDIDATE_LIMIT: usize = 32;
const OBSERVATION_KEY_LIMIT: usize = 512;
const OBSERVATION_TARGET_LIMIT: usize = 253;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ObservedTarget {
    group: String,
    kind: String,
    namespace: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObservedEffective {
    policy_key: String,
    load_balancer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PreparedObservation {
    generation: u64,
    complete: bool,
    target: ObservedTarget,
    candidates: Vec<String>,
    effective: Option<ObservedEffective>,
}

fn append_observation_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if body
        .len()
        .checked_add(chunk.len())
        .is_none_or(|len| len > OBSERVATION_BODY_LIMIT)
    {
        return Err("prepared policy observation body exceeds limit".to_string());
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn parse_prepared_observation(status: reqwest::StatusCode, body: &[u8]) -> Result<PreparedObservation, String> {
    if !status.is_success() {
        return Err(format!("prepared policy observation HTTP status {status}"));
    }
    if body.len() > OBSERVATION_BODY_LIMIT {
        return Err("prepared policy observation body exceeds limit".to_string());
    }
    let envelope: Value = serde_json::from_slice(body).map_err(|e| format!("invalid policy observation JSON: {e}"))?;
    if envelope.get("success").and_then(Value::as_bool) != Some(true)
        || envelope.get("error").is_some_and(|error| !error.is_null())
    {
        return Err("prepared policy observation returned an error body".to_string());
    }
    let data = envelope
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| "policy observation missing data object".to_string())?;
    // Serde permits missing Option fields; require their explicit wire presence.
    if !data.contains_key("effective")
        || data.get("effective").is_some_and(|effective| {
            !effective.is_null()
                && effective
                    .as_object()
                    .is_none_or(|fields| !fields.contains_key("loadBalancer"))
        })
    {
        return Err("policy observation missing effective fields".to_string());
    }
    let observation: PreparedObservation = serde_json::from_value(Value::Object(data.clone()))
        .map_err(|e| format!("invalid prepared policy observation: {e}"))?;
    let target = &observation.target;
    if !observation.complete
        || observation.candidates.len() > OBSERVATION_CANDIDATE_LIMIT
        || target.namespace.is_empty()
        || target.name.is_empty()
        || target.namespace.len() > OBSERVATION_TARGET_LIMIT
        || target.name.len() > OBSERVATION_TARGET_LIMIT
        || !matches!(
            (target.group.as_str(), target.kind.as_str()),
            ("", "Service") | ("edgion.io", "EdgionBackend")
        )
        || observation.candidates.iter().enumerate().any(|(i, key)| {
            key.is_empty() || key.len() > OBSERVATION_KEY_LIMIT || observation.candidates[..i].contains(key)
        })
    {
        return Err("incomplete or invalid prepared policy metadata".to_string());
    }
    match &observation.effective {
        Some(effective)
            if observation.candidates.contains(&effective.policy_key)
                && effective
                    .load_balancer
                    .as_deref()
                    .is_none_or(|algorithm| matches!(algorithm, "Ewma" | "LeastConn" | "ConsistentHash")) => {}
        None if observation.candidates.is_empty() => {}
        _ => return Err("incoherent prepared candidate/effective policy evidence".to_string()),
    }
    Ok(observation)
}

async fn read_prepared_observation(
    client: &reqwest::Client,
    target: &ObservedTarget,
) -> Result<PreparedObservation, String> {
    let mut request = client
        .get(format!("{}/api/v1/debug/backend-traffic-policy", gw_admin_base()))
        .query(&[
            ("group", target.group.as_str()),
            ("kind", target.kind.as_str()),
            ("namespace", target.namespace.as_str()),
            ("name", target.name.as_str()),
        ]);
    if let Some(token) = gw_admin_token() {
        request = request.bearer_auth(token);
    }
    let mut response = request
        .send()
        .await
        .map_err(|e| format!("policy observation request failed: {e}"))?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("policy observation body failed: {e}"))?
    {
        append_observation_chunk(&mut body, &chunk)?;
    }
    parse_prepared_observation(status, &body)
}

struct ConflictExpectation {
    target: ObservedTarget,
    winner: String,
    loser: String,
}

#[derive(Clone, Copy)]
enum PreparedPhase {
    Winner,
    Conflict,
    Promotion { after_generation: u64 },
}

impl ConflictExpectation {
    fn phase_ready(&self, observation: &PreparedObservation, phase: PreparedPhase) -> Result<bool, String> {
        if observation.target != self.target {
            return Err("policy observation typed target mismatch".to_string());
        }
        let has_winner = observation.candidates.contains(&self.winner);
        let has_loser = observation.candidates.contains(&self.loser);
        let (expected_key, algorithm, members_ready) = match phase {
            PreparedPhase::Winner => (&self.winner, "Ewma", has_winner && observation.candidates.len() == 1),
            PreparedPhase::Conflict => (
                &self.winner,
                "Ewma",
                has_winner && has_loser && observation.candidates.len() == 2,
            ),
            PreparedPhase::Promotion { after_generation } => (
                &self.loser,
                "LeastConn",
                has_loser
                    && !has_winner
                    && observation.candidates.len() == 1
                    && observation.generation > after_generation,
            ),
        };
        if !members_ready {
            return Ok(false);
        }
        if observation.effective.as_ref().is_some_and(|effective| {
            effective.policy_key == *expected_key && effective.load_balancer.as_deref() == Some(algorithm)
        }) {
            Ok(true)
        } else {
            Err(format!(
                "prepared policy arbitration mismatch: expected {expected_key} ({algorithm}), got {:?}",
                observation.effective
            ))
        }
    }
}

async fn poll_prepared_conflict<F, Fut>(
    expected: &ConflictExpectation,
    phase: PreparedPhase,
    deadline: tokio::time::Instant,
    mut observe: F,
) -> Result<PreparedObservation, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<PreparedObservation, String>>,
{
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err("prepared policy observation deadline expired".to_string());
        }
        let observation = tokio::time::timeout_at(deadline, observe())
            .await
            .map_err(|_| "prepared policy observation reader timed out".to_string())??;
        if tokio::time::Instant::now() >= deadline {
            return Err("prepared policy observation deadline expired".to_string());
        }
        if expected.phase_ready(&observation, phase)? {
            return Ok(observation);
        }
        tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + Duration::from_millis(100))).await;
    }
}

fn prepared_phase_deadline(shared: tokio::time::Instant) -> tokio::time::Instant {
    shared.min(tokio::time::Instant::now() + Duration::from_secs(6))
}

fn service_key(name: &str) -> String {
    format!("{}/{}", NS, name)
}

fn lb_policy_yaml(name: &str, svc: &str, lb_body: &str) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: {name}\n  namespace: {NS}\nspec:\n  targetRefs:\n    - kind: Service\n      name: {svc}\n{lb_body}"
    )
}

async fn cleanup_policies(ctx: &TestContext, names: &[&str]) {
    for n in names {
        let _ = ctx.delete_resource(KIND, NS, n).await;
    }
}

fn service_yaml(name: &str) -> String {
    format!(
        "apiVersion: v1\nkind: Service\nmetadata:\n  name: {name}\n  namespace: {NS}\nspec:\n  ports:\n    - port: 8080\n      protocol: TCP\n"
    )
}

async fn prepare_services(ctx: &TestContext, names: &[&str]) -> Result<(), String> {
    for name in names {
        ctx.apply_yaml(&service_yaml(name))
            .await
            .map_err(|e| format!("apply Service {name} failed: {e}"))?;
    }
    Ok(())
}

async fn cleanup_services(ctx: &TestContext, names: &[&str]) {
    for name in names {
        let _ = ctx.delete_resource("Service", NS, name).await;
    }
}

impl EdgionBackendTrafficPolicyTestSuite {
    // (3) LB algorithm selection: each LB type resolves into the gateway
    // store's lb_policies map, and a service with no policy has no entry
    // (RoundRobin engine default).
    fn test_lb_store_mapping() -> TestCase {
        TestCase::new(
            "lb_store_mapping",
            "LeastConn/Ewma/ConsistentHash policies resolve in the gateway store; no-policy = RoundRobin (absent)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = admin_client();
                    let names = ["ebtp-lc-policy", "ebtp-ewma-policy", "ebtp-ch-policy"];
                    let services = ["ebtp-lc-svc", "ebtp-ewma-svc", "ebtp-ch-svc", "ebtp-nopolicy-svc"];
                    cleanup_policies(&ctx, &names).await;
                    if let Err(e) = prepare_services(&ctx, &services).await {
                        cleanup_services(&ctx, &services).await;
                        return TestResult::failed(start.elapsed(), e);
                    }

                    let apply = [
                        lb_policy_yaml("ebtp-lc-policy", "ebtp-lc-svc", "  loadBalancer:\n    type: LeastConn\n"),
                        lb_policy_yaml("ebtp-ewma-policy", "ebtp-ewma-svc", "  loadBalancer:\n    type: Ewma\n"),
                        lb_policy_yaml(
                            "ebtp-ch-policy",
                            "ebtp-ch-svc",
                            "  loadBalancer:\n    type: ConsistentHash\n    consistentHash:\n      hashOn: header\n      key: x-user-id\n",
                        ),
                    ];
                    for y in &apply {
                        if let Err(e) = ctx.apply_yaml(y).await {
                            cleanup_policies(&ctx, &names).await;
                            cleanup_services(&ctx, &services).await;
                            return TestResult::failed(start.elapsed(), format!("apply failed: {}", e));
                        }
                    }

                    let (ok_lc, got_lc) =
                        poll_lb(&client, &service_key("ebtp-lc-svc"), |v| v == Some("LeastConn"), 30).await;
                    let (ok_ewma, got_ewma) =
                        poll_lb(&client, &service_key("ebtp-ewma-svc"), |v| v == Some("Ewma"), 30).await;
                    let (ok_ch, got_ch) = poll_lb(
                        &client,
                        &service_key("ebtp-ch-svc"),
                        |v| v.is_some_and(|s| s.starts_with("ConsistentHash")),
                        30,
                    )
                    .await;
                    // no-policy service must be absent (RoundRobin default).
                    let (ok_none, got_none) =
                        poll_lb(&client, &service_key("ebtp-nopolicy-svc"), |v| v.is_none(), 5).await;

                    cleanup_policies(&ctx, &names).await;
                    cleanup_services(&ctx, &services).await;

                    if ok_lc && ok_ewma && ok_ch && ok_none {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "LB store mapping: LeastConn={:?}, Ewma={:?}, ConsistentHash={:?}, no-policy={:?}",
                                got_lc, got_ewma, got_ch, got_none
                            ),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "LB store mapping mismatch: LeastConn(ok={},got={:?}) Ewma(ok={},got={:?}) ConsistentHash(ok={},got={:?}) no-policy(ok={},got={:?})",
                                ok_lc, got_lc, ok_ewma, got_ewma, ok_ch, got_ch, ok_none, got_none
                            ),
                        )
                    }
                })
            },
        )
    }

    // (6) Conflict: two policies target the same Service; the oldest (applied
    // first) wins. Deleting the winner promotes the loser.
    fn test_conflict_oldest_wins_and_promotion() -> TestCase {
        TestCase::new(
            "conflict_oldest_wins_and_promotion",
            "Two policies on one Service: oldest wins; deleting the winner promotes the loser",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = admin_client();
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
                    let nonce = uuid::Uuid::new_v4().simple().to_string();
                    let svc = format!("ebtp-conflict-svc-{nonce}");
                    // Both lexical roles and apply order agree on the older winner.
                    let winner = format!("ebtp-conflict-a-win-{nonce}");
                    let loser = format!("ebtp-conflict-z-lose-{nonce}");
                    let expected = ConflictExpectation {
                        target: ObservedTarget {
                            group: String::new(),
                            kind: "Service".to_string(),
                            namespace: NS.to_string(),
                            name: svc.clone(),
                        },
                        winner: service_key(&winner),
                        loser: service_key(&loser),
                    };
                    let outcome: Result<(), String> = async {
                        prepare_services(&ctx, &[&svc]).await?;
                        ctx.apply_yaml(&lb_policy_yaml(&winner, &svc, "  loadBalancer:\n    type: Ewma\n"))
                            .await
                            .map_err(|e| format!("apply winner failed: {e}"))?;
                        poll_prepared_conflict(
                            &expected,
                            PreparedPhase::Winner,
                            prepared_phase_deadline(deadline),
                            || read_prepared_observation(&client, &expected.target),
                        )
                        .await?;

                        ctx.apply_yaml(&lb_policy_yaml(&loser, &svc, "  loadBalancer:\n    type: LeastConn\n"))
                            .await
                            .map_err(|e| format!("apply loser failed: {e}"))?;
                        // Winner deletion is reachable only after both current prepared
                        // candidates and the actual older winner coexist in one snapshot.
                        let conflict = poll_prepared_conflict(
                            &expected,
                            PreparedPhase::Conflict,
                            prepared_phase_deadline(deadline),
                            || read_prepared_observation(&client, &expected.target),
                        )
                        .await?;
                        ctx.delete_resource(KIND, NS, &winner)
                            .await
                            .map_err(|e| format!("delete winner failed: {e}"))?;
                        poll_prepared_conflict(
                            &expected,
                            PreparedPhase::Promotion {
                                after_generation: conflict.generation,
                            },
                            prepared_phase_deadline(deadline),
                            || read_prepared_observation(&client, &expected.target),
                        )
                        .await?;
                        Ok(())
                    }
                    .await;

                    cleanup_policies(&ctx, &[&winner, &loser]).await;
                    cleanup_services(&ctx, &[&svc]).await;
                    match outcome {
                        Ok(()) => TestResult::passed_with_message(start.elapsed(),
                            "prepared coexistence preserved oldest Ewma; deletion promoted the intended LeastConn policy".to_string()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    // (5) Hot reload: create applies, edit re-applies, delete clears the store.
    fn test_hot_reload_create_edit_delete() -> TestCase {
        TestCase::new(
            "hot_reload_create_edit_delete",
            "Create applies, edit re-applies (LB flip), delete returns the store to the RoundRobin default",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = admin_client();
                    let name = "ebtp-hot-policy";
                    let svc = "ebtp-hot-svc";
                    cleanup_policies(&ctx, &[name]).await;
                    if let Err(e) = prepare_services(&ctx, &[svc]).await {
                        cleanup_services(&ctx, &[svc]).await;
                        return TestResult::failed(start.elapsed(), e);
                    }

                    // Create -> LeastConn.
                    if let Err(e) = ctx
                        .apply_yaml(&lb_policy_yaml(name, svc, "  loadBalancer:\n    type: LeastConn\n"))
                        .await
                    {
                        cleanup_policies(&ctx, &[name]).await;
                        cleanup_services(&ctx, &[svc]).await;
                        return TestResult::failed(start.elapsed(), format!("create apply failed: {}", e));
                    }
                    let (created, _) = poll_lb(&client, &service_key(svc), |v| v == Some("LeastConn"), 30).await;
                    if !created {
                        cleanup_policies(&ctx, &[name]).await;
                        cleanup_services(&ctx, &[svc]).await;
                        return TestResult::failed(start.elapsed(), "create did not resolve to LeastConn".to_string());
                    }

                    // Edit -> Ewma (same name).
                    if let Err(e) = ctx
                        .apply_yaml(&lb_policy_yaml(name, svc, "  loadBalancer:\n    type: Ewma\n"))
                        .await
                    {
                        cleanup_policies(&ctx, &[name]).await;
                        cleanup_services(&ctx, &[svc]).await;
                        return TestResult::failed(start.elapsed(), format!("edit apply failed: {}", e));
                    }
                    let (edited, _) = poll_lb(&client, &service_key(svc), |v| v == Some("Ewma"), 30).await;
                    if !edited {
                        cleanup_policies(&ctx, &[name]).await;
                        cleanup_services(&ctx, &[svc]).await;
                        return TestResult::failed(start.elapsed(), "edit did not flip store to Ewma".to_string());
                    }

                    // Delete -> entry removed (RoundRobin default).
                    if let Err(e) = ctx.delete_resource(KIND, NS, name).await {
                        cleanup_services(&ctx, &[svc]).await;
                        return TestResult::failed(start.elapsed(), format!("delete failed: {}", e));
                    }
                    let (deleted, got) = poll_lb(&client, &service_key(svc), |v| v.is_none(), 30).await;
                    cleanup_services(&ctx, &[svc]).await;

                    if deleted {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "hot reload: create(LeastConn) -> edit(Ewma) -> delete(absent)".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("delete did not clear the store entry, got {:?}", got),
                        )
                    }
                })
            },
        )
    }

    // (1)(2)(4) Config round-trip: a full policy (LB + degradeThreshold +
    // healthCheck + outlierDetection) reaches the gateway intact — asserted via
    // the synced-resource list. The data-plane effects of each section are
    // covered by the HealthCheck / CircuitBreaker / DegradeThreshold suites.
    fn test_full_policy_roundtrips_via_configclient() -> TestCase {
        TestCase::new(
            "full_policy_roundtrips_via_configclient",
            "A policy carrying LB+degrade+healthCheck+outlierDetection round-trips to the gateway config store",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = admin_client();
                    let name = "ebtp-full-policy";
                    let svc = "ebtp-full-svc";
                    cleanup_policies(&ctx, &[name]).await;
                    if let Err(e) = prepare_services(&ctx, &[svc]).await {
                        cleanup_services(&ctx, &[svc]).await;
                        return TestResult::failed(start.elapsed(), e);
                    }

                    let body = "  loadBalancer:\n    type: LeastConn\n    degradeThreshold: 40\n  healthCheck:\n    active:\n      type: http\n      path: /health\n      interval: 1s\n      expectedStatuses:\n        - 200\n  outlierDetection:\n    consecutiveErrors: 3\n    ejectionTime: 3s\n";
                    if let Err(e) = ctx.apply_yaml(&lb_policy_yaml(name, svc, body)).await {
                        cleanup_policies(&ctx, &[name]).await;
                        cleanup_services(&ctx, &[svc]).await;
                        return TestResult::failed(start.elapsed(), format!("apply failed: {}", e));
                    }

                    // Wait for the LB section to resolve (proxy for full sync).
                    let (synced, _) = poll_lb(&client, &service_key(svc), |v| v == Some("LeastConn"), 30).await;
                    if !synced {
                        cleanup_policies(&ctx, &[name]).await;
                        cleanup_services(&ctx, &[svc]).await;
                        return TestResult::failed(start.elapsed(), "policy did not sync to the gateway".to_string());
                    }

                    // Assert the full spec round-tripped via the configclient list.
                    let listed = gw_get_json(&client, "/configclient/edgionbackendtrafficpolicy/list").await;
                    cleanup_policies(&ctx, &[name]).await;
                    cleanup_services(&ctx, &[svc]).await;
                    let listed = match listed {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let items = listed
                        .get("data")
                        .and_then(|d| d.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let found = items
                        .iter()
                        .find(|p| p.pointer("/metadata/name").and_then(|n| n.as_str()) == Some(name));
                    let Some(p) = found else {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("policy {} not present in configclient list", name),
                        );
                    };
                    let has_hc = p.pointer("/spec/healthCheck/active").is_some();
                    let has_od = p.pointer("/spec/outlierDetection").is_some();
                    let has_pt = p.pointer("/spec/loadBalancer/degradeThreshold").is_some();
                    let resolved_as_service =
                        p.pointer("/spec/resolvedTargetClass").and_then(Value::as_str) == Some("service");
                    if has_hc && has_od && has_pt && resolved_as_service {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "full policy round-tripped with service target marker: healthCheck+outlierDetection+degradeThreshold all present".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "synced policy mismatch: healthCheck={}, outlierDetection={}, degradeThreshold={}, resolvedTargetClass=service:{}",
                                has_hc, has_od, has_pt, resolved_as_service
                            ),
                        )
                    }
                })
            },
        )
    }

    // (7) Resync survival (closest harness-supported variant): a policy-driven
    // store entry survives a target Service delete+recreate. NOTE: forcing a
    // gateway<->controller reconnect / full resync is not exposed by the shared
    // test harness (a single long-lived gateway is reused across suites); the
    // full-resync path is instead implicitly exercised when the gateway first
    // connects at suite startup. See task-11-report.md for the divergence.
    fn test_resync_service_churn() -> TestCase {
        TestCase::new(
            "resync_service_churn",
            "Policy-driven LB config survives a target Service delete+recreate",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = admin_client();
                    let name = "ebtp-resync-policy";
                    let svc = "ebtp-resync-svc";
                    cleanup_policies(&ctx, &[name]).await;
                    let _ = ctx.delete_resource("Service", NS, svc).await;

                    let svc_yaml = format!(
                        "apiVersion: v1\nkind: Service\nmetadata:\n  name: {svc}\n  namespace: {NS}\nspec:\n  ports:\n    - port: 8080\n      protocol: TCP\n"
                    );
                    if let Err(e) = ctx.apply_yaml(&svc_yaml).await {
                        return TestResult::failed(start.elapsed(), format!("apply Service failed: {}", e));
                    }
                    if let Err(e) = ctx
                        .apply_yaml(&lb_policy_yaml(name, svc, "  loadBalancer:\n    type: LeastConn\n"))
                        .await
                    {
                        cleanup_policies(&ctx, &[name]).await;
                        return TestResult::failed(start.elapsed(), format!("apply policy failed: {}", e));
                    }
                    let (present, _) = poll_lb(&client, &service_key(svc), |v| v == Some("LeastConn"), 30).await;
                    if !present {
                        cleanup_policies(&ctx, &[name]).await;
                        let _ = ctx.delete_resource("Service", NS, svc).await;
                        return TestResult::failed(start.elapsed(), "policy did not resolve before churn".to_string());
                    }

                    // Delete + recreate the target Service; the policy is untouched.
                    let _ = ctx.delete_resource("Service", NS, svc).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if let Err(e) = ctx.apply_yaml(&svc_yaml).await {
                        cleanup_policies(&ctx, &[name]).await;
                        return TestResult::failed(start.elapsed(), format!("recreate Service failed: {}", e));
                    }

                    let (survived, got) = poll_lb(&client, &service_key(svc), |v| v == Some("LeastConn"), 30).await;

                    cleanup_policies(&ctx, &[name]).await;
                    let _ = ctx.delete_resource("Service", NS, svc).await;

                    if survived {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "policy-driven LeastConn config survived Service delete+recreate".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("policy config lost after Service churn, got {:?}", got),
                        )
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for EdgionBackendTrafficPolicyTestSuite {
    fn name(&self) -> &str {
        "EdgionBackendTrafficPolicy"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_lb_store_mapping(),
            Self::test_conflict_oldest_wins_and_promotion(),
            Self::test_hot_reload_create_edit_delete(),
            Self::test_full_policy_roundtrips_via_configclient(),
            Self::test_resync_service_churn(),
        ]
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/client/suites/edgion_backend_traffic_policy/control_plane_test.rs"]
mod tests;
