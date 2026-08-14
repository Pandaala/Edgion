//! MCP backend publication and bearer rotation without a client-facing MCP frontend.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const NAMESPACE: &str = "edgion-test";
const CROSS_NAMESPACE: &str = "edgion-default";
const BACKEND: &str = "mcp-lifecycle-tools";
const SECRET: &str = "mcp-lifecycle-bearer";
const CROSS_SERVICE: &str = "mcp-lifecycle-upstream";
const GRANT: &str = "allow-mcp-lifecycle-dependencies";
static RUNTIME_BASELINE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
enum ExpectedPhase {
    Valid,
    InvalidValue,
    SecretMissing,
    RefNotPermitted,
    WrongVariant,
    Deleted,
}

struct McpBackendPhaseSuite {
    name: &'static str,
    expected: ExpectedPhase,
}

fn initial() -> Box<dyn TestSuite> {
    Box::new(McpBackendPhaseSuite {
        name: "EdgionBackend/MCP/Lifecycle/Initial",
        expected: ExpectedPhase::Valid,
    })
}

fn invalid() -> Box<dyn TestSuite> {
    Box::new(McpBackendPhaseSuite {
        name: "EdgionBackend/MCP/Lifecycle/InvalidBearer",
        expected: ExpectedPhase::InvalidValue,
    })
}

fn missing() -> Box<dyn TestSuite> {
    Box::new(McpBackendPhaseSuite {
        name: "EdgionBackend/MCP/Lifecycle/MissingSecret",
        expected: ExpectedPhase::SecretMissing,
    })
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(McpBackendPhaseSuite {
        name: "EdgionBackend/MCP/Lifecycle/Deleted",
        expected: ExpectedPhase::Deleted,
    })
}

fn ref_not_permitted() -> Box<dyn TestSuite> {
    Box::new(McpBackendPhaseSuite {
        name: "EdgionBackend/MCP/Lifecycle/RefNotPermitted",
        expected: ExpectedPhase::RefNotPermitted,
    })
}

fn wrong_variant() -> Box<dyn TestSuite> {
    Box::new(McpBackendPhaseSuite {
        name: "EdgionBackend/MCP/Lifecycle/WrongVariant",
        expected: ExpectedPhase::WrongVariant,
    })
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "invalid",
        factory: invalid,
    },
    PhaseSuite {
        name: "rotated",
        factory: initial,
    },
    PhaseSuite {
        name: "external",
        factory: initial,
    },
    PhaseSuite {
        name: "missing",
        factory: missing,
    },
    PhaseSuite {
        name: "recreated",
        factory: initial,
    },
    PhaseSuite {
        name: "cross_denied",
        factory: ref_not_permitted,
    },
    PhaseSuite {
        name: "cross_allowed",
        factory: initial,
    },
    PhaseSuite {
        name: "grant_deleted",
        factory: ref_not_permitted,
    },
    PhaseSuite {
        name: "grant_recreated",
        factory: initial,
    },
    PhaseSuite {
        name: "grant_changed_denied",
        factory: ref_not_permitted,
    },
    PhaseSuite {
        name: "grant_changed_repaired",
        factory: initial,
    },
    PhaseSuite {
        name: "wrong_variant",
        factory: wrong_variant,
    },
    PhaseSuite {
        name: "external_after_retarget",
        factory: initial,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
];

fn authed_controller(ctx: &TestContext, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match ctx.admin_token.as_deref() {
        Some(token) if !token.is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

fn authed_gateway(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        Ok(token) if !token.trim().is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

fn condition<'a>(value: &'a Value, type_: &str) -> Option<(&'a str, &'a str)> {
    let conditions = value
        .pointer("/status/conditions")
        .or_else(|| value.pointer("/conditions"))?
        .as_array()?;
    let condition = conditions
        .iter()
        .find(|condition| condition.get("type").and_then(Value::as_str) == Some(type_))?;
    Some((condition.get("status")?.as_str()?, condition.get("reason")?.as_str()?))
}

async fn check_phase(ctx: TestContext, expected: ExpectedPhase) -> TestResult {
    let start = Instant::now();
    let controller_url = format!(
        "http://127.0.0.1:{}/configserver/EdgionBackend?namespace={NAMESPACE}&name={BACKEND}",
        ctx.admin_port
    );
    let controller_response = match authed_controller(&ctx, ctx.http_client.get(&controller_url))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), format!("Controller request failed: {error}")),
    };

    if matches!(expected, ExpectedPhase::Deleted) {
        if controller_response.status().is_success() {
            return TestResult::failed(
                start.elapsed(),
                "deleted MCP backend remains in Controller output".to_string(),
            );
        }
    } else {
        let status = controller_response.status();
        let body: Value = match controller_response.json().await {
            Ok(body) => body,
            Err(error) => return TestResult::failed(start.elapsed(), format!("invalid Controller JSON: {error}")),
        };
        if !status.is_success() || body.get("success") != Some(&Value::Bool(true)) {
            return TestResult::failed(start.elapsed(), format!("Controller returned {status}: {body}"));
        }
        let resource = &body["data"];
        let expected_resolved = match expected {
            ExpectedPhase::Valid => ("True", "ResolvedRefs"),
            ExpectedPhase::InvalidValue => ("False", "InvalidValue"),
            ExpectedPhase::SecretMissing => ("False", "SecretNotFound"),
            ExpectedPhase::RefNotPermitted => ("False", "RefNotPermitted"),
            ExpectedPhase::WrongVariant => ("True", "ResolvedRefs"),
            ExpectedPhase::Deleted => unreachable!(),
        };
        if condition(resource, "Accepted") != Some(("True", "Accepted"))
            || condition(resource, "ResolvedRefs") != Some(expected_resolved)
        {
            return TestResult::failed(
                start.elapsed(),
                format!("unexpected MCP conditions for phase: {resource}"),
            );
        }
    }

    let gateway_base = std::env::var("EDGION_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:12001".to_string());
    let gateway_response = match authed_gateway(ctx.http_client.get(format!(
        "{}/configclient/edgionbackend/list",
        gateway_base.trim_end_matches('/')
    )))
    .send()
    .await
    {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), format!("Gateway request failed: {error}")),
    };
    let gateway_status = gateway_response.status();
    let gateway_body: Value = match gateway_response.json().await {
        Ok(body) => body,
        Err(error) => return TestResult::failed(start.elapsed(), format!("invalid Gateway JSON: {error}")),
    };
    if !gateway_status.is_success() {
        return TestResult::failed(
            start.elapsed(),
            format!("Gateway configclient returned {gateway_status}: {gateway_body}"),
        );
    }
    let stats_response = match authed_gateway(ctx.http_client.get(format!(
        "{}/api/v1/debug/store-stats",
        gateway_base.trim_end_matches('/')
    )))
    .send()
    .await
    {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), format!("Gateway stats request failed: {error}")),
    };
    let stats_status = stats_response.status();
    let stats_body: Value = match stats_response.json().await {
        Ok(body) => body,
        Err(error) => return TestResult::failed(start.elapsed(), format!("invalid Gateway stats JSON: {error}")),
    };
    let Some(runtime_count) = stats_body
        .pointer("/data/edgion_backend_store/count")
        .and_then(Value::as_u64)
    else {
        return TestResult::failed(
            start.elapsed(),
            format!("Gateway stats omit unified backend count: {stats_body}"),
        );
    };
    if !stats_status.is_success() {
        return TestResult::failed(
            start.elapsed(),
            format!("Gateway stats returned {stats_status}: {stats_body}"),
        );
    }
    let expected_count = RUNTIME_BASELINE.load(Ordering::SeqCst)
        + u64::from(matches!(expected, ExpectedPhase::Valid | ExpectedPhase::WrongVariant));
    if runtime_count != expected_count {
        return TestResult::failed(
            start.elapsed(),
            format!("unified MCP runtime count is {runtime_count}, expected {expected_count}"),
        );
    }
    let backend = gateway_body["data"].as_array().and_then(|items| {
        items.iter().find(|item| {
            item.pointer("/metadata/namespace").and_then(Value::as_str) == Some(NAMESPACE)
                && item.pointer("/metadata/name").and_then(Value::as_str) == Some(BACKEND)
        })
    });
    if matches!(expected, ExpectedPhase::Deleted) {
        if backend.is_some() {
            return TestResult::failed(
                start.elapsed(),
                "deleted MCP backend remains in Gateway cache".to_string(),
            );
        }
    } else {
        let Some(backend) = backend else {
            return TestResult::failed(start.elapsed(), "MCP backend is missing from Gateway cache".to_string());
        };
        let rendered = backend.to_string();
        if [
            "mcp-lifecycle-token-a",
            "mcp-lifecycle-token-b",
            "mcp-lifecycle-cross-token",
            "bad token",
        ]
        .iter()
        .any(|secret| rendered.contains(secret))
        {
            return TestResult::failed(
                start.elapsed(),
                "Gateway configclient leaked MCP bearer material".to_string(),
            );
        }
        if matches!(expected, ExpectedPhase::WrongVariant) {
            if backend.pointer("/spec/a2a").is_none() || backend.pointer("/spec/mcp").is_some() {
                return TestResult::failed(
                    start.elapsed(),
                    "wrong-variant phase did not publish the replacement A2A generation".to_string(),
                );
            }
        } else if matches!(expected, ExpectedPhase::Valid)
            && backend
                .pointer("/spec/mcp/authentication/bearerToken/secret")
                .and_then(Value::as_str)
                != Some("[redacted]")
        {
            return TestResult::failed(start.elapsed(), "resolved MCP bearer is not redacted".to_string());
        }
    }

    TestResult::passed_with_message(start.elapsed(), "MCP backend phase converged".to_string())
}

#[async_trait]
impl TestSuite for McpBackendPhaseSuite {
    fn name(&self) -> &str {
        self.name
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let expected = self.expected;
        vec![TestCase::new(
            "mcp_backend_status_and_redaction",
            "MCP backend status and redacted Gateway cache match the current phase",
            move |ctx| Box::pin(check_phase(ctx, expected)),
        )]
    }
}

async fn cleanup(ctx: &ScenarioCtx) {
    for (kind, namespace, name) in [
        ("EdgionBackend", NAMESPACE, BACKEND),
        ("Secret", NAMESPACE, SECRET),
        ("Secret", CROSS_NAMESPACE, SECRET),
        ("Service", CROSS_NAMESPACE, CROSS_SERVICE),
        ("ReferenceGrant", CROSS_NAMESPACE, GRANT),
    ] {
        let _ = ctx.test_ctx.delete_resource(kind, namespace, name).await;
    }
}

async fn gateway_runtime_count(ctx: &ScenarioCtx) -> anyhow::Result<u64> {
    let response = authed_gateway(ctx.test_ctx.http_client.get(format!(
        "{}/api/v1/debug/store-stats",
        ctx.gateway_url.trim_end_matches('/')
    )))
    .send()
    .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    anyhow::ensure!(status.is_success(), "Gateway stats returned {status}: {body}");
    body.pointer("/data/edgion_backend_store/count")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("Gateway stats omit unified backend count: {body}"))
}

async fn gateway_backend_published(ctx: &ScenarioCtx) -> anyhow::Result<bool> {
    let response = authed_gateway(ctx.test_ctx.http_client.get(format!(
        "{}/configclient/edgionbackend/list",
        ctx.gateway_url.trim_end_matches('/')
    )))
    .send()
    .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    anyhow::ensure!(status.is_success(), "Gateway configclient returned {status}: {body}");
    let items = body["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Gateway configclient response has no data array: {body}"))?;
    Ok(items.iter().any(|item| {
        item.pointer("/metadata/namespace").and_then(Value::as_str) == Some(NAMESPACE)
            && item.pointer("/metadata/name").and_then(Value::as_str) == Some(BACKEND)
    }))
}

/// Wait until the Gateway no longer publishes this scenario's backend.
///
/// The static integration conf loader also loads this scenario's fixture
/// directory, so the backend is normally still resident when the scenario
/// starts. A baseline captured before the Gateway observes the cleanup delete
/// would make the first apply an update instead of an add, and the phase
/// assertion could never reach `baseline + 1`. EdgionBackend events are
/// delivered in order, so observing our own removal also proves every earlier
/// EdgionBackend event — including residue from a preceding scenario — has
/// already been applied to the unified runtime store.
async fn wait_backend_withdrawn(ctx: &ScenarioCtx) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last_error = None;
    loop {
        match gateway_backend_published(ctx).await {
            Ok(false) => return Ok(()),
            Ok(true) => {}
            Err(error) => last_error = Some(error),
        }
        if Instant::now() >= deadline {
            return match last_error {
                Some(error) => Err(anyhow::anyhow!(
                    "timed out waiting for Gateway to withdraw {NAMESPACE}/{BACKEND}; last error: {error:#}"
                )),
                None => Err(anyhow::anyhow!(
                    "timed out waiting for Gateway to withdraw {NAMESPACE}/{BACKEND}"
                )),
            };
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn run_inner(ctx: &ScenarioCtx) -> anyhow::Result<()> {
    ctx.apply_dir("initial").await?;
    ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(15)).await?;

    ctx.apply_dir("invalid").await?;
    ctx.wait_phase(SCENARIO, "invalid", Duration::from_secs(15)).await?;

    ctx.apply_dir("rotated").await?;
    ctx.wait_phase(SCENARIO, "rotated", Duration::from_secs(15)).await?;

    ctx.apply_dir("external").await?;
    ctx.wait_phase(SCENARIO, "external", Duration::from_secs(15)).await?;

    ctx.test_ctx
        .delete_resource("Secret", NAMESPACE, SECRET)
        .await
        .map_err(anyhow::Error::msg)?;
    ctx.wait_phase(SCENARIO, "missing", Duration::from_secs(15)).await?;

    ctx.apply_dir("rotated").await?;
    ctx.wait_phase(SCENARIO, "recreated", Duration::from_secs(15)).await?;

    ctx.apply_dir("cross_denied").await?;
    ctx.wait_phase(SCENARIO, "cross_denied", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("cross_allowed").await?;
    ctx.wait_phase(SCENARIO, "cross_allowed", Duration::from_secs(15))
        .await?;

    ctx.test_ctx
        .delete_resource("ReferenceGrant", CROSS_NAMESPACE, GRANT)
        .await
        .map_err(anyhow::Error::msg)?;
    ctx.wait_phase(SCENARIO, "grant_deleted", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("cross_allowed").await?;
    ctx.wait_phase(SCENARIO, "grant_recreated", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("grant_changed_denied").await?;
    ctx.wait_phase(SCENARIO, "grant_changed_denied", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("cross_allowed").await?;
    ctx.wait_phase(SCENARIO, "grant_changed_repaired", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("wrong_variant").await?;
    ctx.wait_phase(SCENARIO, "wrong_variant", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("external").await?;
    ctx.test_ctx
        .delete_resource("ReferenceGrant", CROSS_NAMESPACE, GRANT)
        .await
        .map_err(anyhow::Error::msg)?;
    ctx.wait_phase(SCENARIO, "external_after_retarget", Duration::from_secs(15))
        .await?;

    ctx.test_ctx
        .delete_resource("EdgionBackend", NAMESPACE, BACKEND)
        .await
        .map_err(anyhow::Error::msg)?;
    ctx.wait_phase(SCENARIO, "deleted", Duration::from_secs(15)).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        cleanup(&ctx).await;
        wait_backend_withdrawn(&ctx).await?;
        RUNTIME_BASELINE.store(gateway_runtime_count(&ctx).await?, Ordering::SeqCst);
        let result = run_inner(&ctx).await;
        cleanup(&ctx).await;
        result
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "McpBackendLifecycle",
    conf_path: "EdgionBackend/Mcp/DynamicTest",
    run,
    phases: PHASES,
};
