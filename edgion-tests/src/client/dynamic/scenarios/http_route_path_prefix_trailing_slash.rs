//! Prefix spellings must preserve selection across a published backend change.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

struct PrefixSuite(Option<u16>);

impl TestSuite for PrefixSuite {
    fn name(&self) -> &str {
        "HttpRoutePathPrefixTrailingSlash"
    }
    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Basic")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let port = self.0;
        vec![TestCase::new(
            "prefix_generation",
            "Check prefix eligibility, precedence, and serving backend",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx, port).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(20))]
    }
}

async fn check(ctx: &TestContext, port: Option<u16>) -> Result<()> {
    for path in ["/echo", "/echo/", "/echo/child", "/echox", "/Echo"] {
        let response = ctx
            .http_client
            .get(format!("{}{path}", ctx.http_url()))
            .header("host", "prefix-slash.example.com")
            .timeout(Duration::from_secs(3))
            .send()
            .await?;
        if let Some(port) = port.filter(|_| path != "/echox" && path != "/Echo") {
            ensure!(
                response.status().as_u16() == 200,
                "{path}: expected success, got {}",
                response.status()
            );
            ensure!(
                response.text().await?.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
                "{path}: wrong backend generation or prefix precedence"
            );
        } else {
            ensure!(
                response.status().as_u16() == 404,
                "{path}: expected no route, got {}",
                response.status()
            );
        }
    }
    Ok(())
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(PrefixSuite(None))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(PrefixSuite(Some(30001)))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(PrefixSuite(Some(30002)))
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
    let token = edgion_cli::cli::auto_auth::resolve_token(None);
    let mut resources = vec![
        ("HTTPRoute", "prefix-slash-z-winner"),
        ("HTTPRoute", "prefix-slash-a-loser"),
        ("Service", "prefix-slash-30001"),
        ("Service", "prefix-slash-30002"),
        ("Service", "prefix-slash-30003"),
    ];
    if std::env::var("EDGION_TEST_K8S_MODE").as_deref() != Ok("true") {
        resources.extend([
            ("EndpointSlice", "prefix-slash-30001"),
            ("EndpointSlice", "prefix-slash-30002"),
            ("EndpointSlice", "prefix-slash-30003"),
        ]);
    }
    let mut failures = Vec::new();
    for (kind, name) in resources {
        let mut request = ctx
            .test_ctx
            .http_client
            .delete(format!(
                "{}/api/v1/namespaced/{kind}/edgion-test/{name}",
                ctx.controller_url.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(5));
        if let Some(token) = &token {
            request = request.bearer_auth(token.as_str());
        }
        match request.send().await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("{kind}/{name}: HTTP {}", response.status())),
            Err(error) => failures.push(format!("{kind}/{name}: {error}")),
        }
    }
    ensure!(failures.is_empty(), "cleanup failed: {}", failures.join("; "));
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await?;
            if std::env::var("EDGION_TEST_K8S_MODE").as_deref() != Ok("true") {
                ctx.apply_dir("bootstrap/").await?;
            }
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            let updated_path = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
                "updates/updated/"
            } else {
                "updated/"
            };
            ctx.apply_dir(updated_path).await?;
            ctx.wait_phase(SCENARIO, "updated", Duration::from_secs(20)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            Ok(())
        }
        .await;
        let cleanup = async {
            cleanup(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await
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
    id: "HttpRoutePathPrefixTrailingSlash",
    conf_path: "HTTPRoute/PathPrefixTrailingSlash/DynamicTest",
    run,
    phases: PHASES,
};
