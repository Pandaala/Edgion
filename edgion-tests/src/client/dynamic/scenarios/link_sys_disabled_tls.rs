//! Disabled TLS references must not block fresh Redis clients or replacement generations.
//! Invalid endpoint generations must report `Accepted=False` and retain the last-known-good client.
//! Uses the shared Redis service in each test environment; never starts another topology.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const NS: &str = "default";
const NAME: &str = "redis-disabled-tls-dynamic";
const PROBES: [&str; 2] = ["redis-disabled-tls-probe-0", "redis-disabled-tls-probe-1"];
const AUTH: &str = "redis-disabled-tls-auth";
const TIMEOUT: Duration = Duration::from_secs(30);

fn namespace() -> &'static str {
    if std::env::var("EDGION_TEST_K8S_MODE").is_ok_and(|mode| matches!(mode.as_str(), "true" | "1" | "yes")) {
        "edgion-default"
    } else {
        NS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Absent,
    Initial,
    Updated,
    InvalidEndpoint,
    Blocked,
    Recovered,
}

struct TlsSuite(Phase);

impl TestSuite for TlsSuite {
    fn name(&self) -> &str {
        "LinkSysDisabledTls"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "tls_publication",
            "Check current status and the database used by the runtime client",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx, phase).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{phase:?}: {error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(25))]
    }
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(TlsSuite(Phase::Absent))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(TlsSuite(Phase::Initial))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(TlsSuite(Phase::Updated))
}
fn invalid_endpoint() -> Box<dyn TestSuite> {
    Box::new(TlsSuite(Phase::InvalidEndpoint))
}
fn blocked() -> Box<dyn TestSuite> {
    Box::new(TlsSuite(Phase::Blocked))
}
fn recovered() -> Box<dyn TestSuite> {
    Box::new(TlsSuite(Phase::Recovered))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "invalid_endpoint",
        factory: invalid_endpoint,
    },
    PhaseSuite {
        name: "blocked",
        factory: blocked,
    },
    PhaseSuite {
        name: "recovered",
        factory: recovered,
    },
];

fn gateway_url(ctx: &TestContext) -> String {
    std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host))
        .trim_end_matches('/')
        .to_owned()
}

fn authed(ctx: &TestContext, request: reqwest::RequestBuilder, gateway: bool) -> reqwest::RequestBuilder {
    let gateway_token = gateway
        .then(|| std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok())
        .flatten();
    let request = request.timeout(Duration::from_secs(3));
    match gateway_token.as_deref().or(ctx.admin_token.as_deref()) {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

async fn resource(ctx: &TestContext, name: &str, gateway: bool) -> Result<Option<Value>> {
    let ns = namespace();
    let base = if gateway { gateway_url(ctx) } else { ctx.admin_api_url() };
    let prefix = if gateway { "configclient" } else { "configserver" };
    let response = authed(
        ctx,
        ctx.http_client
            .get(format!("{base}/{prefix}/LinkSys?namespace={ns}&name={name}")),
        gateway,
    )
    .send()
    .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = response.status();
    let body: Value = response.json().await?;
    if gateway && status.is_success() && body["success"] == false && body["error"] == "LinkSys not found" {
        return Ok(None);
    }
    ensure!(
        status.is_success() && body["success"] == true,
        "LinkSys query failed: {status}"
    );
    Ok(body.get("data").filter(|data| !data.is_null()).cloned())
}

async fn redis(ctx: &TestContext, name: &str, operation: &str, payload: Option<Value>) -> Result<Value> {
    let ns = namespace();
    let url = format!(
        "{}/api/v1/testing/link-sys/redis/{ns}_{name}/{operation}",
        gateway_url(ctx)
    );
    let request = match payload {
        Some(payload) => ctx.http_client.post(url).json(&payload),
        None => ctx.http_client.get(url),
    };
    let response = authed(ctx, request, true).send().await?;
    let status = response.status();
    let body: Value = response.json().await?;
    ensure!(
        status.is_success() && body["success"] == true,
        "Redis {name}/{operation} failed: {body}"
    );
    Ok(body["data"].clone())
}

async fn check_database(ctx: &TestContext, expected: usize) -> Result<()> {
    // Fresh keys per polling attempt distinguish actual DB selection from a
    // still-live old client. Probe clients use fixed DBs and never change.
    let key = format!("disabled-tls-{}", uuid::Uuid::new_v4());
    let result: Result<()> = async {
        for (db, name) in PROBES.iter().enumerate() {
            redis(
                ctx,
                name,
                "set",
                Some(json!({
                    "key": key, "value": format!("db-{db}"), "ttl_seconds": 60
                })),
            )
            .await?;
        }
        let data = redis(ctx, NAME, &format!("get/{key}"), None).await?;
        ensure!(
            data["value"] == format!("db-{expected}"),
            "runtime client uses the wrong database: {data}"
        );
        Ok(())
    }
    .await;
    let mut cleanup_errors = Vec::new();
    for name in PROBES {
        if let Err(error) = redis(ctx, name, "del", Some(json!({"keys": [&key]}))).await {
            cleanup_errors.push(error.to_string());
        }
    }
    result?;
    ensure!(
        cleanup_errors.is_empty(),
        "Redis marker cleanup failed: {cleanup_errors:?}"
    );
    Ok(())
}

async fn check(ctx: &TestContext, phase: Phase) -> Result<()> {
    let ns = namespace();
    if phase == Phase::Absent {
        for name in [NAME, PROBES[0], PROBES[1]] {
            for gateway in [false, true] {
                ensure!(
                    resource(ctx, name, gateway).await?.is_none(),
                    "LinkSys {name} remains published"
                );
            }
        }
        let response = authed(
            ctx,
            ctx.http_client
                .get(format!("{}/api/v1/testing/link-sys/redis/clients", gateway_url(ctx))),
            true,
        )
        .send()
        .await?
        .error_for_status()?;
        let body: Value = response.json().await?;
        ensure!(body["success"] == true, "Redis client inventory query failed");
        let clients = body["data"].as_array().context("missing client inventory")?;
        for name in [NAME, PROBES[0], PROBES[1]] {
            ensure!(
                !clients.iter().any(|client| client == &format!("{ns}/{name}")),
                "runtime client {name} remains"
            );
        }
        return Ok(());
    }
    let invalid_endpoint = phase == Phase::InvalidEndpoint;
    let enabled = phase == Phase::Blocked;
    let configured_db = usize::from(phase == Phase::Updated);
    for gateway in [false, true] {
        let resource = resource(ctx, NAME, gateway).await?.context("target LinkSys absent")?;
        let config = &resource["spec"]["config"];
        ensure!(
            config["db"] == configured_db && config["tls"]["enabled"] == enabled,
            "stale LinkSys configuration"
        );
        ensure!(
            config["endpoints"].as_array().map(Vec::len) == Some(if invalid_endpoint { 2 } else { 1 }),
            "stale Redis endpoints"
        );
        if !gateway {
            let conditions = resource["status"]["conditions"]
                .as_array()
                .context("missing status conditions")?;
            for (kind, expected) in [
                ("Accepted", if invalid_endpoint { "False" } else { "True" }),
                ("ResolvedRefs", if enabled { "False" } else { "True" }),
            ] {
                let condition = conditions
                    .iter()
                    .find(|condition| condition["type"] == kind)
                    .context("missing condition")?;
                ensure!(condition["status"] == expected, "unexpected {kind}: {condition}");
                if kind == "Accepted" && invalid_endpoint {
                    ensure!(
                        condition["reason"] == "Invalid",
                        "unexpected Accepted reason: {condition}"
                    );
                    let message = condition["message"].as_str().context("missing Accepted message")?;
                    ensure!(
                        message.contains("endpoints[1] is invalid"),
                        "missing endpoint index: {condition}"
                    );
                    ensure!(
                        !message.contains("not-a-port"),
                        "endpoint input leaked into status: {condition}"
                    );
                }
                if let Some(generation) = resource["metadata"]["generation"].as_i64() {
                    ensure!(condition["observedGeneration"] == generation, "stale {kind} generation");
                }
            }
        }
    }
    // Invalid and unresolved generations request DB 0, but must retain DB 1.
    check_database(
        ctx,
        usize::from(matches!(
            phase,
            Phase::Updated | Phase::InvalidEndpoint | Phase::Blocked
        )),
    )
    .await
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let ns = namespace();
    let mut errors = Vec::new();
    for (kind, name) in [
        ("LinkSys", NAME),
        ("LinkSys", PROBES[0]),
        ("LinkSys", PROBES[1]),
        ("Secret", AUTH),
    ] {
        let response = authed(
            &ctx.test_ctx,
            ctx.test_ctx.http_client.delete(format!(
                "{}/api/v1/namespaced/{kind}/{ns}/{name}",
                ctx.controller_url.trim_end_matches('/')
            )),
            false,
        )
        .send()
        .await;
        match response {
            Ok(response) if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND => {}
            Ok(response) => errors.push(format!("delete {kind}/{name}: {}", response.status())),
            Err(error) => errors.push(format!("delete {kind}/{name}: {error}")),
        }
    }
    ensure!(errors.is_empty(), "cleanup failed: {errors:?}");
    ctx.wait_phase(SCENARIO, "absent", TIMEOUT).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", TIMEOUT).await?;
            ctx.apply_dir("updates/disabled-db-1.yaml").await?;
            ctx.wait_phase(SCENARIO, "updated", TIMEOUT).await?;
            ctx.apply_dir("updates/invalid-endpoint-db-0.yaml").await?;
            ctx.wait_phase(SCENARIO, "invalid_endpoint", TIMEOUT).await?;
            ctx.apply_dir("updates/enabled-db-0.yaml").await?;
            ctx.wait_phase(SCENARIO, "blocked", TIMEOUT).await?;
            ctx.apply_dir("initial/03_LinkSys.yaml").await?;
            ctx.wait_phase(SCENARIO, "recovered", TIMEOUT).await
        }
        .await;
        let cleanup_result = cleanup(&ctx).await;
        match (result, cleanup_result) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "LinkSysDisabledTls",
    conf_path: "LinkSys/Redis/DisabledTls/DynamicTest",
    run,
    phases: PHASES,
};
