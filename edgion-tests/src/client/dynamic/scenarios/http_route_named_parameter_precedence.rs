//! Named-parameter precedence and capture names must survive route publication.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

struct NamedParameterSuite(Option<(u16, &'static str)>);

impl TestSuite for NamedParameterSuite {
    fn name(&self) -> &str {
        "HttpRouteNamedParameterPrecedence"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/NamedParameterPrecedence")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let generation = self.0;
        vec![
            TestCase::new(
                "static_over_named_parameter",
                "A static path outranks a matching named-parameter path",
                move |ctx: TestContext| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let result = check_backend(&ctx, "/named-rank/fixed", generation.map(|_| 30001)).await;
                        match result {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                        }
                    })
                },
            )
            .with_timeout(Duration::from_secs(15)),
            TestCase::new(
                "equal_specificity_capture_generation",
                "Equal path keys use route precedence and expose only current capture names",
                move |ctx: TestContext| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let result = check_capture_generation(&ctx, generation).await;
                        match result {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                        }
                    })
                },
            )
            .with_timeout(Duration::from_secs(15)),
        ]
    }
}

async fn response_body(ctx: &TestContext, path: &str) -> Result<(u16, String)> {
    let response = ctx
        .http_client
        .get(format!("{}{path}", ctx.http_url()))
        .header("host", "named-parameter-precedence.example.com")
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    let status = response.status().as_u16();
    let body = response.text().await?;
    Ok((status, body))
}

async fn check_backend(ctx: &TestContext, path: &str, expected_port: Option<u16>) -> Result<()> {
    let (status, body) = response_body(ctx, path).await?;
    match expected_port {
        Some(port) => {
            ensure!(status == 200, "{path}: expected success, got HTTP {status}");
            ensure!(
                body.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
                "{path}: expected backend {port}, got {body:?}"
            );
        }
        None => ensure!(status == 404, "{path}: expected no route, got HTTP {status}"),
    }
    Ok(())
}

fn echoed_header<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    body.lines().find_map(|line| {
        let (header, value) = line.trim().split_once(':')?;
        header.eq_ignore_ascii_case(name).then_some(value.trim())
    })
}

async fn check_capture_generation(ctx: &TestContext, generation: Option<(u16, &'static str)>) -> Result<()> {
    let path = "/named-capture/value";
    let Some((port, current_name)) = generation else {
        return check_backend(ctx, path, None).await;
    };
    let (status, body) = response_body(ctx, path).await?;
    ensure!(status == 200, "{path}: expected success, got HTTP {status}");
    ensure!(
        body.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
        "{path}: expected backend {port}, got {body:?}"
    );

    let (current_header, stale_header) = if current_name == "short" {
        ("x-capture-short", "x-capture-customer-identifier")
    } else {
        ("x-capture-customer-identifier", "x-capture-short")
    };
    ensure!(
        echoed_header(&body, current_header) == Some("value"),
        "{path}: current capture {current_name:?} was not resolved: {body:?}"
    );
    ensure!(
        echoed_header(&body, stale_header).is_some_and(|value| value.starts_with('$')),
        "{path}: stale capture unexpectedly remained available: {body:?}"
    );
    Ok(())
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(NamedParameterSuite(None))
}

fn initial() -> Box<dyn TestSuite> {
    Box::new(NamedParameterSuite(Some((30001, "short"))))
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(NamedParameterSuite(Some((30002, "customerIdentifier"))))
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
];

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let resources = [
        ("HTTPRoute", "named-static-z-winner"),
        ("HTTPRoute", "named-static-a-loser"),
        ("HTTPRoute", "named-equal-a-winner"),
        ("HTTPRoute", "named-equal-z-loser"),
        ("EdgionPlugins", "named-capture-observer"),
        ("EndpointSlice", "named-precedence-30001"),
        ("EndpointSlice", "named-precedence-30002"),
        ("EndpointSlice", "named-precedence-30003"),
        ("Service", "named-precedence-30001"),
        ("Service", "named-precedence-30002"),
        ("Service", "named-precedence-30003"),
    ];
    let mut failures = Vec::new();
    for (kind, name) in resources {
        match client.delete(kind, Some("edgion-test"), name).await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("{kind}/{name}: HTTP {}", response.status())),
            Err(error) => failures.push(format!("{kind}/{name}: {error}")),
        }
    }
    ensure!(failures.is_empty(), "cleanup failed: {}", failures.join("; "));
    Ok(())
}

async fn cleanup_gateway(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let response = client
        .delete("Gateway", Some("edgion-test"), "named-parameter-precedence-gateway")
        .await?;
    ensure!(
        response.status().is_success() || response.status().as_u16() == 404,
        "gateway cleanup failed: {}",
        response.status()
    );
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            ctx.apply_dir("bootstrap/").await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            ctx.apply_dir("updated/").await?;
            ctx.wait_phase(SCENARIO, "updated", Duration::from_secs(20)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await
        }
        .await;
        let cleanup = async {
            cleanup(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await?;
            cleanup_gateway(&ctx).await
        }
        .await;
        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "HttpRouteNamedParameterPrecedence",
    conf_path: "HTTPRoute/NamedParameterPrecedence/DynamicTest",
    run,
    phases: PHASES,
};
