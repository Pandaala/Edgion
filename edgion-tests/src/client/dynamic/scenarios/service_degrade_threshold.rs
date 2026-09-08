//! Hot reload of Service degrade overrides against an observably unhealthy pool.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::counter_value;
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

const NAME: &str = "service-degrade-dynamic";
const HOST: &str = "service-degrade-dynamic.example.com";
const SERVICE: &str = "edgion-test/service-degrade-dynamic";

struct DegradeSuite(u16);

async fn check(ctx: &TestContext, expected: u16) -> Result<()> {
    let metrics = ctx.metrics_client();
    let before = if expected == 404 {
        0.0
    } else {
        counter_value(
            &metrics.fetch_raw_metrics().await?,
            "edgion_lb_degrade_selections_total",
            &[("service", SERVICE)],
        )
    };
    let response = ctx
        .http_client
        .get(format!("{}/echo", ctx.http_url()))
        .header("host", HOST)
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    ensure!(
        response.status().as_u16() == expected,
        "expected {expected}, got {}",
        response.status()
    );
    if expected == 404 {
        return Ok(());
    }
    if expected == 200 {
        ensure!(
            response.text().await?.starts_with("Server: 0.0.0.0:30001\n"),
            "degraded request did not reach the shared backend"
        );
    }
    let after = counter_value(
        &metrics.fetch_raw_metrics().await?,
        "edgion_lb_degrade_selections_total",
        &[("service", SERVICE)],
    );
    if expected == 200 {
        // A successful request alone could observe a temporarily healthy pool.
        // The counter delta proves this request actually bypassed health filtering.
        ensure!(after > before, "successful request did not enter degrade selection");
    } else {
        ensure!(after == before, "disabled threshold still entered degrade selection");
    }
    Ok(())
}

impl TestSuite for DegradeSuite {
    fn name(&self) -> &str {
        "ServiceDegradeThreshold"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/DegradeThreshold")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let expected = self.0;
        vec![TestCase::new(
            "threshold_selection",
            "Verify traffic and actual degrade events",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx, expected).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(25))]
    }
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(DegradeSuite(404))
}
fn disabled() -> Box<dyn TestSuite> {
    Box::new(DegradeSuite(503))
}
fn degraded() -> Box<dyn TestSuite> {
    Box::new(DegradeSuite(200))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "disabled",
        factory: disabled,
    },
    PhaseSuite {
        name: "degraded",
        factory: degraded,
    },
];

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let token = edgion_cli::cli::auto_auth::resolve_token(None);
    let mut failures = Vec::new();
    for kind in ["HTTPRoute", "EdgionBackendTrafficPolicy", "EndpointSlice", "Service"] {
        let mut request = ctx
            .test_ctx
            .http_client
            .delete(format!(
                "{}/api/v1/namespaced/{kind}/edgion-test/{NAME}",
                ctx.controller_url.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(5));
        if let Some(token) = &token {
            request = request.bearer_auth(token.as_str());
        }
        match request.send().await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("{kind}: HTTP {}", response.status())),
            Err(error) => failures.push(format!("{kind}: {error}")),
        }
    }
    ensure!(failures.is_empty(), "cleanup failed: {}", failures.join("; "));
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(30)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "disabled", Duration::from_secs(30)).await?;
            ctx.apply_dir("updates/enabled.yaml").await?;
            ctx.wait_phase(SCENARIO, "degraded", Duration::from_secs(30)).await?;
            ctx.apply_dir("initial/02_Policy.yaml").await?;
            ctx.wait_phase(SCENARIO, "disabled", Duration::from_secs(30)).await
        }
        .await;
        let cleanup_result = async {
            cleanup(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(30)).await
        }
        .await;
        match (result, cleanup_result) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "ServiceDegradeThreshold",
    conf_path: "HTTPRoute/Backend/DegradeThreshold/DynamicTest",
    run,
    phases: PHASES,
};
