//! Header-derived Host allowlists must change atomically with plugin generations.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

struct HostSuite(Option<&'static str>);

impl TestSuite for HostSuite {
    fn name(&self) -> &str {
        "ProxyRewriteHostDynamic"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let allowed = self.0;
        vec![TestCase::new(
            "host_allowlist",
            "Verify forwarded Host and fail-closed inputs after reload",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result: Result<()> = async {
                        for values in [
                            vec![],
                            vec![""],
                            vec!["api.internal"],
                            vec!["admin.internal"],
                            vec!["api.internal.evil"],
                            vec!["user@api.internal"],
                            vec!["api.internal", "api.internal"],
                        ] {
                            let mut request = ctx
                                .http_client
                                .get(format!("{}/echo", ctx.http_url()))
                                .header("host", "proxy-host-from-header.example.com");
                            for value in &values {
                                request = request.header("x-backend-host", *value);
                            }
                            let response = request.send().await?;
                            let expected = match allowed {
                                None => 404,
                                Some(host) if values == vec![host] => 200,
                                Some(_) => 400,
                            };
                            ensure!(
                                response.status().as_u16() == expected,
                                "input {values:?}: expected {expected}, got {}",
                                response.status()
                            );
                            if expected == 200 {
                                let body = response.text().await?;
                                let echoed = body.lines().find_map(|line| {
                                    let (name, value) = line.trim().split_once(':')?;
                                    name.eq_ignore_ascii_case("host").then_some(value.trim())
                                });
                                ensure!(echoed == allowed, "origin Host differs from selected input");
                            }
                        }
                        Ok(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(HostSuite(None))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(HostSuite(Some("api.internal")))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(HostSuite(Some("admin.internal")))
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
    for kind in ["HTTPRoute", "EdgionPlugins"] {
        let response = client
            .delete(kind, Some("edgion-default"), "proxy-host-from-header")
            .await?;
        ensure!(
            response.status().is_success() || response.status().as_u16() == 404,
            "Host fixture cleanup failed: {}",
            response.status()
        );
    }
    ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            ctx.apply_dir("updates/").await?;
            ctx.wait_phase(SCENARIO, "updated", Duration::from_secs(20)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await
        }
        .await;
        let cleanup = cleanup(&ctx).await;
        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "ProxyRewriteHostDynamic",
    conf_path: "EdgionPlugins/ProxyRewrite/DynamicTest",
    run,
    phases: PHASES,
};
