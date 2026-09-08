//! D0065: RegionRoute validation over generic ingest, conf_sync, traffic, and management HTTP.
//! The scenario owns only rrv-* resources and reuses the shared EdgionPlugins listener.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const NS: &str = "edgion-test";
const OVERLAY: &str = "rrv-overlay";
const CONSUMERS: [&str; 2] = ["rrv-small", "rrv-large"];
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Absent,
    Initial,
    Replacement,
    Overlap,
    SmallModulo,
    BothModulos,
    MalformedEndpoint,
    SelfTarget,
    MissingTarget,
    Cycle,
    Empty,
    Missing,
    Disabled,
    WrongType,
    Omitted,
    OneHop,
    Chain,
}

fn region(name: &str, low: u16, high: u16, port: u16) -> Value {
    json!({"name": name, "hashRange": [low, high],
        "backendEndpoint": format!("127.0.0.1:{port}"), "tls": false})
}

fn base_regions() -> Value {
    json!([region("base", 0, 99, 30001)])
}

impl Phase {
    fn resource(self) -> Option<Value> {
        if matches!(self, Self::Absent | Self::Missing) {
            return None;
        }
        let mut regions = json!([
            region("a", 0, 29, 30001),
            region("b", 30, 59, 30002),
            region("c", 60, 99, 30003),
        ]);
        match self {
            Self::Replacement => {
                regions[0]["hashRange"] = json!([30, 59]);
                regions[1]["hashRange"] = json!([0, 29]);
            }
            Self::Overlap => regions[1]["hashRange"] = json!([29, 59]),
            Self::SmallModulo => regions[2]["hashRange"] = json!([60, 999]),
            Self::BothModulos => regions[2]["hashRange"] = json!([60, 1000]),
            Self::MalformedEndpoint => regions[2]["backendEndpoint"] = json!("127.0.0.1:bad"),
            Self::SelfTarget => regions[0]["failoverTo"] = json!("a"),
            Self::MissingTarget => regions[0]["failoverTo"] = json!("missing"),
            Self::Cycle => {
                regions[0]["failoverTo"] = json!("b");
                regions[1]["failoverTo"] = json!("a");
            }
            Self::Empty => regions = json!([]),
            Self::OneHop | Self::Chain => {
                regions[0]["failoverTo"] = json!("b");
                if self == Self::Chain {
                    regions[1]["failoverTo"] = json!("c");
                }
            }
            _ => {}
        }
        let data = match self {
            Self::WrongType => json!({"type": "Selector", "config": {"active": "unused"}}),
            Self::Omitted => json!({"type": "RegionRouteOverride", "config": {}}),
            _ => json!({"type": "RegionRouteOverride", "config": {"regions": regions}}),
        };
        Some(json!({
            "apiVersion": "edgion.io/v1", "kind": "EdgionConfigData",
            "metadata": {"name": OVERLAY, "namespace": NS},
            "spec": {"enable": self != Self::Disabled, "data": data},
        }))
    }

    fn standalone_invalid(self) -> bool {
        matches!(
            self,
            Self::Overlap
                | Self::MalformedEndpoint
                | Self::SelfTarget
                | Self::MissingTarget
                | Self::Cycle
                | Self::Empty
        )
    }

    fn applies(self, small: bool) -> bool {
        matches!(self, Self::Initial | Self::Replacement | Self::OneHop | Self::Chain)
            || self == Self::SmallModulo && !small
    }

    fn refs_resolved(self, small: bool) -> bool {
        self.applies(small) || self == Self::Omitted
    }

    fn selected(self, small: bool) -> &'static str {
        if !self.applies(small) {
            "base"
        } else if matches!(self, Self::Replacement | Self::OneHop | Self::Chain) {
            "b"
        } else {
            "a"
        }
    }
}

struct PhaseSuiteImpl(Phase);

impl TestSuite for PhaseSuiteImpl {
    fn name(&self) -> &str {
        "RegionRouteValidation"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "region_table_generation",
            "Assert status, published resource, effective tables, and independently observed traffic",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check_phase(&ctx, phase).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{phase:?}: {error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}

// Keep the standard static phase inventory so wait_phase and K8s barriers see every transition.
macro_rules! phases {
    ($($name:ident => $phase:ident),+ $(,)?) => {
        $(fn $name() -> Box<dyn TestSuite> { Box::new(PhaseSuiteImpl(Phase::$phase)) })+
        static PHASES: &[PhaseSuite] = &[
            $(PhaseSuite { name: stringify!($name), factory: $name }),+
        ];
    };
}

phases! {
    absent => Absent, initial => Initial, replacement => Replacement,
    overlap => Overlap, small_modulo => SmallModulo, both_modulos => BothModulos,
    malformed_endpoint => MalformedEndpoint, self_target => SelfTarget,
    missing_target => MissingTarget, cycle => Cycle, empty => Empty,
    missing => Missing, disabled => Disabled, wrong_type => WrongType,
    omitted => Omitted, one_hop => OneHop, chain => Chain,
}

fn authed(ctx: &TestContext, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match ctx.admin_token.as_deref() {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

fn gateway_url(ctx: &TestContext) -> String {
    std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .map(|url| url.trim_end_matches('/').to_string())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host))
}

async fn get_data(ctx: &TestContext, url: &str, gateway: bool, missing_kind: Option<&str>) -> Result<Option<Value>> {
    let request = ctx.http_client.get(url).timeout(Duration::from_secs(3));
    let request = if gateway {
        match std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok() {
            Some(token) => request.bearer_auth(token),
            None => authed(ctx, request),
        }
    } else {
        authed(ctx, request)
    };
    let response = request.send().await.with_context(|| format!("GET {url}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = response.status();
    let body: Value = response.json().await?;
    // The Gateway resource API reports absence in a 200 error envelope. Match
    // its exact missing-kind response, never treat an arbitrary error as deletion.
    if gateway
        && status == reqwest::StatusCode::OK
        && body["success"] == false
        && missing_kind.is_some_and(|kind| body["error"] == format!("{kind} not found"))
    {
        return Ok(None);
    }
    ensure!(
        status.is_success() && body["success"] == true,
        "GET {url}: {status} {body}"
    );
    Ok(body.get("data").filter(|value| !value.is_null()).cloned())
}

async fn resource(ctx: &TestContext, kind: &str, name: &str, gateway: bool) -> Result<Option<Value>> {
    let (base, prefix) = if gateway {
        (gateway_url(ctx), "configclient")
    } else {
        (ctx.admin_api_url(), "configserver")
    };
    get_data(
        ctx,
        &format!("{base}/{prefix}/{kind}?namespace={NS}&name={name}"),
        gateway,
        gateway.then_some(kind),
    )
    .await
}

fn condition(resource: &Value, type_: &str, expected: bool) -> Result<()> {
    let conditions = resource
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .context("missing status.conditions")?;
    let condition = conditions
        .iter()
        .find(|c| c["type"] == type_)
        .with_context(|| format!("missing {type_} condition"))?;
    let status = if expected { "True" } else { "False" };
    ensure!(
        condition["status"] == status,
        "expected {type_}={status}, got {condition}"
    );
    if let Some(generation) = resource.pointer("/metadata/generation").and_then(Value::as_i64) {
        ensure!(
            condition["observedGeneration"] == generation,
            "stale {type_} generation"
        );
    }
    Ok(())
}

fn assert_overlay(actual: &Value, expected: &Value) -> Result<()> {
    for pointer in [
        "/spec/enable",
        "/spec/data/type",
        "/spec/data/config/regions",
        "/spec/data/config/active",
    ] {
        ensure!(
            actual.pointer(pointer) == expected.pointer(pointer),
            "published {pointer} differs: {actual}"
        );
    }
    Ok(())
}

async fn check_phase(ctx: &TestContext, phase: Phase) -> Result<()> {
    let expected = phase.resource();
    let controller = resource(ctx, "EdgionConfigData", OVERLAY, false).await?;
    let gateway = resource(ctx, "EdgionConfigData", OVERLAY, true).await?;
    match &expected {
        Some(expected) => {
            let controller = controller.as_ref().context("Controller overlay absent")?;
            assert_overlay(controller, expected).context("Controller overlay")?;
            condition(controller, "Accepted", !phase.standalone_invalid())?;
            assert_overlay(gateway.as_ref().context("Gateway overlay absent")?, expected)
                .context("Gateway publication")?;
        }
        None => ensure!(
            controller.is_none() && gateway.is_none(),
            "overlay deletion not published"
        ),
    }

    let effective = get_data(
        ctx,
        &format!("{}/api/v1/region-routes/effective", ctx.admin_api_url()),
        false,
        None,
    )
    .await?
    .context("effective API has no data")?;
    let entries = effective.as_array().context("effective data is not an array")?;
    ensure!(
        !entries
            .iter()
            .any(|e| e["namespace"] == NS && e["pluginName"] == "rrv-bad"),
        "effective API exposed an invalid base"
    );
    if phase == Phase::Absent {
        for name in ["rrv-small", "rrv-large", "rrv-bad"] {
            for gateway in [false, true] {
                ensure!(
                    resource(ctx, "EdgionPlugins", name, gateway).await?.is_none(),
                    "plugin {name} remains"
                );
            }
            ensure!(
                !entries.iter().any(|e| e["namespace"] == NS && e["pluginName"] == name),
                "effective entry {name} remains"
            );
        }
        for name in CONSUMERS {
            for gateway in [false, true] {
                ensure!(
                    resource(ctx, "HTTPRoute", name, gateway).await?.is_none(),
                    "route {name} remains"
                );
            }
        }
        return Ok(());
    }

    let invalid = resource(ctx, "EdgionPlugins", "rrv-bad", false)
        .await?
        .context("invalid base sample missing")?;
    condition(&invalid, "Accepted", false)?;
    for (index, name) in CONSUMERS.iter().enumerate() {
        let small = index == 0;
        let plugin = resource(ctx, "EdgionPlugins", name, false)
            .await?
            .context("consumer missing")?;
        condition(&plugin, "Accepted", true)?;
        condition(&plugin, "ResolvedRefs", phase.refs_resolved(small))?;
        let matches: Vec<_> = entries
            .iter()
            .filter(|e| e["namespace"] == NS && e["pluginName"] == *name)
            .collect();
        ensure!(matches.len() == 1, "expected one effective entry for {name}");
        let entry = matches[0];
        ensure!(
            entry["overrideApplied"] == phase.applies(small),
            "wrong overrideApplied for {name}"
        );
        let expected_regions = if phase.applies(small) {
            expected.as_ref().context("applied overlay absent")?["spec"]["data"]["config"]["regions"].clone()
        } else {
            base_regions()
        };
        ensure!(
            entry["regions"] == expected_regions,
            "wrong effective table for {name}: {entry}"
        );
        check_traffic(ctx, name, phase, small).await?;
    }
    Ok(())
}

async fn check_traffic(ctx: &TestContext, name: &str, phase: Phase, small: bool) -> Result<()> {
    let target = phase.selected(small);
    let trace = format!("rrv-{}", uuid::Uuid::new_v4());
    let mut request = ctx
        .http_client
        .get(format!("{}/echo", ctx.http_url()))
        .header("host", format!("{name}.example.com"))
        .header("X-Region-Hash", "10")
        .header("x-trace-id", &trace)
        .header("access_log", "test_store")
        .timeout(Duration::from_secs(3));
    if matches!(phase, Phase::OneHop | Phase::Chain) {
        request = request.header("X-Direct-Region", "a");
    }
    let response = request.send().await?;
    let status = response.status().as_u16();
    if target == "base" {
        ensure!(status == 200, "{name}: base pass-through returned {status}");
        let body = response.text().await?;
        ensure!(
            body.contains("Server: 0.0.0.0:30001"),
            "base response did not reach the shared backend"
        );
    } else {
        // Preserve the production post-resolution loopback guard. The access log proves
        // the selected target; an error status alone cannot distinguish a stale overlay.
        ensure!(
            matches!(status, 500 | 502),
            "{name}: external loopback target returned {status}"
        );
        let _ = response.bytes().await?;
    }
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(&trace, 5, 100)
        .await?;
    ensure!(entry.route_name() == Some(name), "traffic used a different route");
    let stages = entry.data["stage_logs"].as_array().context("missing stage logs")?;
    let logs: Vec<_> = stages
        .iter()
        .filter_map(|s| s["edgion_plugins"].as_array())
        .flatten()
        .filter(|group| group["name"] == name)
        .filter_map(|group| group["logs"].as_array())
        .flatten()
        .filter(|p| p["name"] == "RegionRoute")
        .collect();
    ensure!(logs.len() == 1, "expected one RegionRoute execution");
    let log = logs[0].to_string();
    if target == "base" {
        ensure!(log.contains("local_region, pass"), "base was not selected: {log}");
    } else {
        let port = if target == "a" { 30001 } else { 30002 };
        ensure!(
            entry.has_diagnostic("error", "BackendLocalhostNotAllowed"),
            "remote selection did not preserve the loopback security rejection"
        );
        ensure!(
            log.contains(&format!("→{target} 127.0.0.1:{port}")),
            "wrong final target: {log}"
        );
    }
    if matches!(phase, Phase::OneHop | Phase::Chain) {
        ensure!(
            log.contains("failover a→b") && !log.contains("failover b→c"),
            "failover must execute exactly one hop: {log}"
        );
    }
    Ok(())
}

async fn publish(ctx: &ScenarioCtx, phase: Phase, name: &str) -> Result<()> {
    if let Some(resource) = phase.resource() {
        ctx.test_ctx
            .apply_yaml(&serde_yaml::to_string(&resource)?)
            .await
            .map_err(anyhow::Error::msg)?;
    } else {
        delete_if_present(&ctx.test_ctx, "EdgionConfigData", OVERLAY).await?;
    }
    ctx.wait_phase(SCENARIO, name, TIMEOUT).await
}

async fn stored(ctx: &TestContext, kind: &str, name: &str) -> Result<Option<Value>> {
    // ConfCenter GET returns the resource directly; configserver/configclient use
    // the {success, data} envelope. Keep the persisted-content observation separate.
    let url = format!("{}/api/v1/namespaced/{kind}/{NS}/{name}", ctx.admin_api_url());
    let response = authed(ctx, ctx.http_client.get(&url).timeout(Duration::from_secs(3)))
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = response.status();
    let resource: Value = response.json().await?;
    ensure!(
        status.is_success() && resource["kind"] == kind,
        "GET {url}: {status} {resource}"
    );
    Ok(Some(resource))
}

async fn delete_if_present(ctx: &TestContext, kind: &str, name: &str) -> Result<()> {
    if stored(ctx, kind, name).await?.is_some() {
        ctx.delete_resource(kind, NS, name).await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    for name in CONSUMERS {
        delete_if_present(&ctx.test_ctx, "HTTPRoute", name).await?;
    }
    for name in ["rrv-small", "rrv-large", "rrv-bad"] {
        delete_if_present(&ctx.test_ctx, "EdgionPlugins", name).await?;
    }
    delete_if_present(&ctx.test_ctx, "EdgionConfigData", OVERLAY).await?;
    ctx.wait_phase(SCENARIO, "absent", TIMEOUT).await
}

async fn failover(ctx: &TestContext, from: &str, to: &str, expected_status: u16) -> Result<()> {
    let before = stored(ctx, "EdgionConfigData", OVERLAY)
        .await?
        .context("stored overlay missing")?;
    let response = authed(
        ctx,
        ctx.http_client
            .post(format!("{}/api/v1/cluster-region-routes/failover", ctx.admin_api_url()))
            .timeout(Duration::from_secs(5))
            .json(&json!({"namespace": NS, "name": OVERLAY, "regionName": from, "failoverTo": to})),
    )
    .send()
    .await?;
    let status = response.status().as_u16();
    ensure!(
        status == expected_status,
        "failover {from}->{to}: expected {expected_status}, got {status}"
    );
    if expected_status == 422 {
        let after = stored(ctx, "EdgionConfigData", OVERLAY)
            .await?
            .context("overlay disappeared after 422")?;
        ensure!(before["spec"] == after["spec"], "422 mutated stored spec");
        ensure!(
            before.pointer("/metadata/generation") == after.pointer("/metadata/generation"),
            "422 changed the configuration generation"
        );
        // Kubernetes may concurrently update status and thus resourceVersion; its
        // unchanged spec/generation is the portable no-write observation.
        if std::env::var("EDGION_TEST_K8S_MODE").as_deref() != Ok("true") {
            ensure!(
                before.pointer("/metadata/resourceVersion") == after.pointer("/metadata/resourceVersion"),
                "422 advanced the stored resourceVersion"
            );
        }
    } else {
        let body: Value = response.json().await?;
        ensure!(
            body["success"] == true && body["modified"] == true,
            "failover did not report a write"
        );
    }
    Ok(())
}

async fn run_inner(ctx: &ScenarioCtx) -> Result<()> {
    // Consumers are applied once. Every subsequent mutation targets only the overlay.
    let initial = Phase::Initial.resource().context("initial overlay missing")?;
    ctx.test_ctx
        .apply_yaml(&serde_yaml::to_string(&initial)?)
        .await
        .map_err(anyhow::Error::msg)?;
    ctx.apply_dir("initial").await?;
    ctx.wait_phase(SCENARIO, "initial", TIMEOUT).await?;
    let mut consumer_specs = Vec::new();
    for name in CONSUMERS {
        consumer_specs.push(
            stored(&ctx.test_ctx, "EdgionPlugins", name)
                .await?
                .context("consumer absent")?["spec"]
                .clone(),
        );
    }

    for (phase, name) in [
        (Phase::Overlap, "overlap"),
        (Phase::SmallModulo, "small_modulo"),
        (Phase::BothModulos, "both_modulos"),
        (Phase::MalformedEndpoint, "malformed_endpoint"),
        (Phase::SelfTarget, "self_target"),
        (Phase::MissingTarget, "missing_target"),
        (Phase::Cycle, "cycle"),
        (Phase::Empty, "empty"),
        (Phase::Missing, "missing"),
        (Phase::Disabled, "disabled"),
        (Phase::WrongType, "wrong_type"),
        (Phase::Omitted, "omitted"),
    ] {
        // A distinct previous target b detects accidental retention of a last-good overlay.
        publish(ctx, Phase::Replacement, "replacement").await?;
        publish(ctx, phase, name).await?;
        if phase.standalone_invalid() && phase != Phase::Empty {
            // Also validate the complete table when clearing an already absent field.
            failover(&ctx.test_ctx, "c", "", 422).await?;
        }
        publish(ctx, Phase::Initial, "initial").await?;
    }

    failover(&ctx.test_ctx, "a", "a", 422).await?;
    failover(&ctx.test_ctx, "a", "missing", 422).await?;
    failover(&ctx.test_ctx, "a", "b", 200).await?;
    ctx.wait_phase(SCENARIO, "one_hop", TIMEOUT).await?;
    failover(&ctx.test_ctx, "b", "c", 200).await?;
    ctx.wait_phase(SCENARIO, "chain", TIMEOUT).await?;
    failover(&ctx.test_ctx, "c", "a", 422).await?;
    ctx.wait_phase(SCENARIO, "chain", TIMEOUT).await?;
    for (name, spec) in CONSUMERS.iter().zip(consumer_specs) {
        let current = stored(&ctx.test_ctx, "EdgionPlugins", name)
            .await?
            .context("consumer disappeared")?;
        ensure!(
            current["spec"] == spec,
            "consumer {name} changed during overlay-only transitions"
        );
    }
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        cleanup(&ctx).await.context("initial cleanup")?;
        let result = run_inner(&ctx).await;
        let cleanup_result = cleanup(&ctx).await;
        match (result, cleanup_result) {
            (Err(error), Err(cleanup)) => Err(error.context(format!("cleanup also failed: {cleanup:#}"))),
            (Err(error), _) => Err(error),
            (Ok(()), cleanup) => cleanup,
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "RegionRouteValidation",
    conf_path: "EdgionPlugins/RegionRoute/DynamicTest",
    run,
    phases: PHASES,
};
