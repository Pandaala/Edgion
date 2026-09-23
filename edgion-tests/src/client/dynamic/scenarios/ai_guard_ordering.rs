//! D0082: AiGuard ordering must fail closed across Controller/Gateway hot reload.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{test_backend_hosts, TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const NS: &str = "edgion-test";
const NAME: &str = "ai-guard-ordering-dynamic";
const HOST: &str = "ai-guard-ordering-dynamic.example.com";
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy)]
enum Phase {
    Valid,
    Invalid,
}

struct OrderingSuite(Phase);

impl TestSuite for OrderingSuite {
    fn name(&self) -> &str {
        "AiGuardOrderingDynamic"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/SingleProvider")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "controller_status_gateway_rejection_and_provider_count",
            "Check the published ordering generation at the HTTP boundary",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check_phase(&ctx, phase).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )]
    }
}

fn valid() -> Box<dyn TestSuite> {
    Box::new(OrderingSuite(Phase::Valid))
}

fn invalid() -> Box<dyn TestSuite> {
    Box::new(OrderingSuite(Phase::Invalid))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "valid",
        factory: valid,
    },
    PhaseSuite {
        name: "invalid",
        factory: invalid,
    },
];

async fn check_status(ctx: &TestContext, valid: bool) -> Result<()> {
    let mut request = ctx.http_client.get(format!(
        "{}/configserver/EdgionPlugins?namespace={NS}&name={NAME}",
        ctx.admin_api_url()
    ));
    if let Some(token) = ctx.admin_token.as_deref() {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.context("Controller resource lookup failed")?;
    ensure!(
        response.status().is_success(),
        "Controller resource lookup returned {}",
        response.status()
    );
    let body: Value = response
        .json()
        .await
        .context("Controller resource response was not JSON")?;
    ensure!(body["success"] == true, "Controller resource lookup failed: {body}");
    let resource = &body["data"];
    let accepted = crate::dynamic::status::conditions(resource)?
        .iter()
        .find(|condition| condition["type"] == "Accepted")
        .context("EdgionPlugins Accepted condition is missing")?;
    ensure!(
        accepted["status"] == if valid { "True" } else { "False" },
        "wrong Accepted condition: {accepted}"
    );
    if !valid {
        ensure!(accepted["reason"] == "Invalid", "wrong invalid reason: {accepted}");
        ensure!(
            accepted["message"]
                .as_str()
                .unwrap_or_default()
                .contains("AiGuard must be ordered before AiProxy"),
            "ordering diagnostic is missing: {accepted}"
        );
    }
    if let Some(generation) = resource["metadata"]["generation"].as_i64() {
        ensure!(
            accepted["observedGeneration"] == generation,
            "stale Accepted condition: {accepted}"
        );
    }
    Ok(())
}

async fn provider_hits(ctx: &TestContext, trace: &str) -> Result<u64> {
    let mut total = 0_u64;
    for host in test_backend_hosts(ctx) {
        let response = ctx
            .http_client
            .get(format!("http://{host}:30001/ai-resilience/count/{trace}"))
            .send()
            .await
            .with_context(|| format!("provider count lookup failed for {host}"))?;
        ensure!(
            response.status().is_success(),
            "provider count lookup returned {}",
            response.status()
        );
        let body: Value = response.json().await.context("provider count response was not JSON")?;
        let count = body["half_open_fallback"]
            .as_u64()
            .context("provider count is missing")?;
        total = total.checked_add(count).context("provider count overflow")?;
    }
    Ok(total)
}

async fn request(ctx: &TestContext, chat: bool, skip_guard: bool, trace: &str) -> Result<reqwest::Response> {
    let path = if chat { "/v1/chat/completions" } else { "/health" };
    let mut request = if chat {
        ctx.http_client.post(format!("{}{path}", ctx.http_url())).json(&json!({
            "model": "fixture-model",
            "stream": false,
            "messages": [{"role": "user", "content": "fixed test prompt"}],
        }))
    } else {
        ctx.http_client.get(format!("{}{path}", ctx.http_url()))
    };
    request = request
        .header("host", HOST)
        .header("connection", "close")
        .header("traceparent", crate::access_log_client::traceparent_for_marker(trace))
        .header("x-resilience-fixture-id", trace);
    if skip_guard {
        request = request.header("x-skip-ai-guard", "true");
    }
    request.send().await.context("Gateway request failed")
}

async fn check_phase(ctx: &TestContext, phase: Phase) -> Result<()> {
    let valid = matches!(phase, Phase::Valid);
    check_status(ctx, valid).await?;
    let cases: &[(bool, bool)] = if valid {
        &[(true, false), (true, true)]
    } else {
        &[(true, false), (true, true), (false, true)]
    };
    for &(chat, skip_guard) in cases {
        let trace = format!("ai-guard-ordering-{}", uuid::Uuid::new_v4());
        let response = request(ctx, chat, skip_guard, &trace).await?;
        let status = response.status();
        let body = response.text().await.context("Gateway response body failed")?;
        let expected = if valid { 200 } else { 500 };
        ensure!(
            status.as_u16() == expected,
            "expected {expected}, got {status} for chat={chat} skip={skip_guard}: {body}"
        );
        if !valid {
            ensure!(
                !body.contains("AiGuard must be ordered") && !body.contains("ai-provider-test-key"),
                "invalid resource leaked configuration detail: {body}"
            );
        }
        let hits = provider_hits(ctx, &trace).await?;
        let expected_hits = u64::from(valid && chat);
        ensure!(
            hits == expected_hits,
            "expected {expected_hits} provider calls, got {hits} for chat={chat} skip={skip_guard}"
        );
    }
    Ok(())
}

async fn apply_backend(ctx: &ScenarioCtx) -> Result<()> {
    let host = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
        "edgion-test-server.edgion-test.svc.cluster.local"
    } else {
        "127.0.0.1"
    };
    let backend = json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionBackend",
        "metadata": {"name": NAME, "namespace": NS},
        "spec": {"ai": {
            "provider": "OpenAICompatible",
            "endpoint": format!("http://{host}:30001/ai/resilience-half-open-fallback"),
            "allowInsecureHttp": true,
            "credentialPool": {"credentials": [{"name": "primary", "secretRef": {"name": NAME}}]},
            "models": [{"name": "fixture-model"}],
        }}
    });
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    crate::dynamic::apply::apply_yaml_documents(
        &client,
        &ctx.resolve("generated-backend.yaml"),
        &serde_yaml::to_string(&backend)?,
    )
    .await
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    for kind in ["HTTPRoute", "EdgionPlugins", "EdgionBackend", "Secret"] {
        let response = client.delete(kind, Some(NS), NAME).await?;
        ensure!(
            response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND,
            "deleting {kind}/{NAME} returned {}",
            response.status()
        );
    }
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            apply_backend(&ctx).await?;
            ctx.apply_dir("initial").await?;
            ctx.wait_phase(SCENARIO, "valid", TIMEOUT).await?;
            ctx.apply_dir("updates/to_invalid").await?;
            ctx.wait_phase(SCENARIO, "invalid", TIMEOUT).await?;
            ctx.apply_dir("initial/03_EdgionPlugins.yaml").await?;
            ctx.wait_phase(SCENARIO, "valid", TIMEOUT).await
        }
        .await;
        let cleaned = cleanup(&ctx).await;
        match (result, cleaned) {
            (Ok(()), cleaned) => cleaned,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "AiGuardOrderingDynamic",
    conf_path: "EdgionPlugins/AiGuardOrdering/DynamicTest",
    run,
    phases: PHASES,
};
