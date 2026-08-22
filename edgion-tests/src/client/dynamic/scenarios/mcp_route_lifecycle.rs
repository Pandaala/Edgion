//! MCP HTTPRoute classification lifecycle without data-plane request activation.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const NAMESPACE: &str = "edgion-test";
const ROUTE: &str = "mcp-route-lifecycle";
const BACKEND: &str = "mcp-route-lifecycle-tools";
static GATEWAY_URL: OnceLock<String> = OnceLock::new();

fn gateway_url() -> &'static str {
    GATEWAY_URL
        .get_or_init(|| {
            std::env::var("EDGION_GATEWAY_ADMIN_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:12001".to_string())
        })
        .as_str()
}

#[derive(Clone, Copy, Debug)]
enum ExpectedPhase {
    InvalidMcp,
    OrdinaryA2a,
    ValidMcp,
    MissingBackend,
}

struct McpRoutePhaseSuite {
    name: &'static str,
    expected: ExpectedPhase,
}

fn invalid_initial() -> Box<dyn TestSuite> {
    Box::new(McpRoutePhaseSuite {
        name: "HTTPRoute/MCP/Lifecycle/InvalidInitial",
        expected: ExpectedPhase::InvalidMcp,
    })
}

fn ordinary_a2a() -> Box<dyn TestSuite> {
    Box::new(McpRoutePhaseSuite {
        name: "HTTPRoute/MCP/Lifecycle/OrdinaryA2A",
        expected: ExpectedPhase::OrdinaryA2a,
    })
}

fn invalid_restored() -> Box<dyn TestSuite> {
    Box::new(McpRoutePhaseSuite {
        name: "HTTPRoute/MCP/Lifecycle/InvalidRestored",
        expected: ExpectedPhase::InvalidMcp,
    })
}

fn valid_exact() -> Box<dyn TestSuite> {
    Box::new(McpRoutePhaseSuite {
        name: "HTTPRoute/MCP/Lifecycle/ValidExact",
        expected: ExpectedPhase::ValidMcp,
    })
}

fn missing_backend() -> Box<dyn TestSuite> {
    Box::new(McpRoutePhaseSuite {
        name: "HTTPRoute/MCP/Lifecycle/MissingBackend",
        expected: ExpectedPhase::MissingBackend,
    })
}

fn recreated_backend() -> Box<dyn TestSuite> {
    Box::new(McpRoutePhaseSuite {
        name: "HTTPRoute/MCP/Lifecycle/RecreatedBackend",
        expected: ExpectedPhase::ValidMcp,
    })
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "invalid_initial",
        factory: invalid_initial,
    },
    PhaseSuite {
        name: "ordinary_a2a",
        factory: ordinary_a2a,
    },
    PhaseSuite {
        name: "invalid_restored",
        factory: invalid_restored,
    },
    PhaseSuite {
        name: "valid_exact",
        factory: valid_exact,
    },
    PhaseSuite {
        name: "missing_backend",
        factory: missing_backend,
    },
    PhaseSuite {
        name: "recreated_backend",
        factory: recreated_backend,
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

async fn controller_resource(ctx: &TestContext, kind: &str, name: &str) -> Result<Option<Value>> {
    let url = format!(
        "http://127.0.0.1:{}/configserver/{kind}?namespace={NAMESPACE}&name={name}",
        ctx.admin_port
    );
    let response = authed_controller(ctx, ctx.http_client.get(&url))
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    anyhow::ensure!(status.is_success(), "GET {url} returned {status}");
    let body: Value = response
        .json()
        .await
        .with_context(|| format!("GET {url} returned invalid JSON"))?;
    if body.get("success") != Some(&Value::Bool(true)) {
        return Ok(None);
    }
    Ok(body.get("data").cloned())
}

async fn gateway_resource(ctx: &TestContext, endpoint: &str, name: &str) -> Result<Option<Value>> {
    let url = format!("{}/configclient/{endpoint}/list", gateway_url().trim_end_matches('/'));
    let response = authed_gateway(ctx.http_client.get(&url))
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .with_context(|| format!("GET {url} returned invalid JSON"))?;
    anyhow::ensure!(status.is_success(), "GET {url} returned {status}: {body}");
    let items = body["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Gateway {endpoint} response has no data array: {body}"))?;
    Ok(items
        .iter()
        .find(|item| {
            item.pointer("/metadata/namespace").and_then(Value::as_str) == Some(NAMESPACE)
                && item.pointer("/metadata/name").and_then(Value::as_str) == Some(name)
        })
        .cloned())
}

fn has_condition(value: &Value, type_: &str, status: &str, reason: &str) -> bool {
    value
        .pointer("/status/parents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|parent| parent.get("conditions").and_then(Value::as_array))
        .flatten()
        .any(|condition| {
            condition.get("type").and_then(Value::as_str) == Some(type_)
                && condition.get("status").and_then(Value::as_str) == Some(status)
                && condition.get("reason").and_then(Value::as_str) == Some(reason)
        })
}

fn has_invalid_index(value: &Value) -> bool {
    value
        .pointer("/spec/invalidRuleIndices")
        .and_then(Value::as_array)
        .is_some_and(|indices| indices == &[Value::from(0)])
}

fn assert_route_view(label: &str, route: &Value, expected: ExpectedPhase) -> Result<(), String> {
    let invalid = has_invalid_index(route);
    match expected {
        ExpectedPhase::InvalidMcp => {
            if !invalid || !has_condition(route, "Accepted", "False", "UnsupportedValue") {
                return Err(format!(
                    "{label} did not reject the sole MCP PathPrefix rule with invalidRuleIndices=[0]: {route}"
                ));
            }
        }
        ExpectedPhase::OrdinaryA2a => {
            if invalid
                || !has_condition(route, "Accepted", "True", "Accepted")
                || !has_condition(route, "ResolvedRefs", "True", "ResolvedRefs")
            {
                return Err(format!(
                    "{label} retained MCP invalid state or unresolved refs after replacement with A2A: {route}"
                ));
            }
        }
        ExpectedPhase::ValidMcp => {
            if invalid
                || !has_condition(route, "Accepted", "True", "Accepted")
                || !has_condition(route, "ResolvedRefs", "True", "ResolvedRefs")
            {
                return Err(format!(
                    "{label} did not accept the exact MCP route with resolved refs: {route}"
                ));
            }
        }
        ExpectedPhase::MissingBackend => {
            if invalid || !has_condition(route, "ResolvedRefs", "False", "BackendNotFound") {
                return Err(format!(
                    "{label} retained MCP invalid state or omitted BackendNotFound after deletion: {route}"
                ));
            }
        }
    }
    Ok(())
}

async fn check_phase(ctx: TestContext, expected: ExpectedPhase) -> TestResult {
    let started = Instant::now();
    let controller = match controller_resource(&ctx, "HTTPRoute", ROUTE).await {
        Ok(Some(route)) => route,
        Ok(None) => {
            return TestResult::failed(
                started.elapsed(),
                "HTTPRoute is missing from Controller output".to_string(),
            );
        }
        Err(error) => {
            return TestResult::failed(
                started.elapsed(),
                format!("Controller HTTPRoute query failed: {error:#}"),
            );
        }
    };
    let gateway = match gateway_resource(&ctx, "httproute", ROUTE).await {
        Ok(Some(route)) => route,
        Ok(None) => {
            return TestResult::failed(
                started.elapsed(),
                "HTTPRoute is missing from Gateway config cache".to_string(),
            );
        }
        Err(error) => {
            return TestResult::failed(started.elapsed(), format!("Gateway HTTPRoute query failed: {error:#}"));
        }
    };

    for (label, route) in [("Controller", &controller), ("Gateway", &gateway)] {
        if let Err(error) = assert_route_view(label, route, expected) {
            return TestResult::failed(started.elapsed(), error);
        }
    }

    TestResult::passed_with_message(
        started.elapsed(),
        format!("MCP HTTPRoute phase {expected:?} converged in Controller and Gateway"),
    )
}

#[async_trait]
impl TestSuite for McpRoutePhaseSuite {
    fn name(&self) -> &str {
        self.name
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let expected = self.expected;
        vec![TestCase::new(
            "controller_and_gateway_route_generation",
            "Controller and Gateway expose the same MCP HTTPRoute lifecycle semantics",
            move |ctx| Box::pin(check_phase(ctx, expected)),
        )]
    }
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let mut delete_errors = Vec::new();
    for (kind, name) in [("HTTPRoute", ROUTE), ("EdgionBackend", BACKEND)] {
        match ctx.test_ctx.delete_resource(kind, NAMESPACE, name).await {
            Ok(()) => {}
            Err(error) if error.contains("404 Not Found") => {}
            Err(error) => delete_errors.push(format!("{kind}/{NAMESPACE}/{name}: {error}")),
        }
    }
    anyhow::ensure!(
        delete_errors.is_empty(),
        "cleanup delete request(s) failed: {}",
        delete_errors.join("; ")
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if controller_resource(&ctx.test_ctx, "HTTPRoute", ROUTE).await?.is_none()
            && controller_resource(&ctx.test_ctx, "EdgionBackend", BACKEND)
                .await?
                .is_none()
            && gateway_resource(&ctx.test_ctx, "httproute", ROUTE).await?.is_none()
            && gateway_resource(&ctx.test_ctx, "edgionbackend", BACKEND)
                .await?
                .is_none()
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for {NAMESPACE}/{ROUTE} and {NAMESPACE}/{BACKEND} cleanup in Controller and Gateway"
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn run_inner(ctx: &ScenarioCtx) -> Result<()> {
    ctx.apply_dir("mcp_backend").await?;
    ctx.apply_dir("invalid_route").await?;
    ctx.wait_phase(SCENARIO, "invalid_initial", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("a2a_backend").await?;
    ctx.wait_phase(SCENARIO, "ordinary_a2a", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("mcp_backend").await?;
    ctx.wait_phase(SCENARIO, "invalid_restored", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("exact_route").await?;
    ctx.wait_phase(SCENARIO, "valid_exact", Duration::from_secs(15)).await?;

    ctx.test_ctx
        .delete_resource("EdgionBackend", NAMESPACE, BACKEND)
        .await
        .map_err(anyhow::Error::msg)?;
    ctx.wait_phase(SCENARIO, "missing_backend", Duration::from_secs(15))
        .await?;

    ctx.apply_dir("mcp_backend").await?;
    ctx.wait_phase(SCENARIO, "recreated_backend", Duration::from_secs(15))
        .await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            run_inner(&ctx).await
        }
        .await;
        let cleanup_result = cleanup(&ctx).await;
        match (result, cleanup_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Err(run_error), Err(cleanup_error)) => Err(anyhow::anyhow!(
                "scenario failed: {run_error:#}; cleanup also failed: {cleanup_error:#}"
            )),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "McpRouteLifecycle",
    conf_path: "HTTPRoute/Mcp/DynamicTest",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_manifests_keep_route_shape_and_backend_variant_explicit() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("integration/conf/HTTPRoute/Mcp/DynamicTest");
        for relative in [
            "mcp_backend/00_EdgionBackend_mcp-route-lifecycle-tools.yaml",
            "a2a_backend/00_EdgionBackend_mcp-route-lifecycle-tools.yaml",
            "invalid_route/00_HTTPRoute_mcp-route-lifecycle.yaml",
            "exact_route/00_HTTPRoute_mcp-route-lifecycle.yaml",
        ] {
            let text = std::fs::read_to_string(root.join(relative)).unwrap();
            serde_yaml::from_str::<Value>(&text)
                .unwrap_or_else(|error| panic!("invalid lifecycle YAML {relative}: {error}"));
        }

        let invalid: Value = serde_yaml::from_str(
            &std::fs::read_to_string(root.join("invalid_route/00_HTTPRoute_mcp-route-lifecycle.yaml")).unwrap(),
        )
        .unwrap();
        let exact: Value = serde_yaml::from_str(
            &std::fs::read_to_string(root.join("exact_route/00_HTTPRoute_mcp-route-lifecycle.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            invalid.pointer("/spec/rules/0/matches/0/path/type"),
            Some(&Value::String("PathPrefix".into()))
        );
        assert_eq!(
            exact.pointer("/spec/rules/0/matches/0/path/type"),
            Some(&Value::String("Exact".into()))
        );
    }
}
