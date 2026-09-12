//! Duplicate header order changes must replace the effective route generation.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::http_route::HttpMatchTestSuite;
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

struct HeaderSuite(Option<(&'static str, u16)>);

impl TestSuite for HeaderSuite {
    fn name(&self) -> &str {
        "HttpRouteDuplicateHeaders"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Match")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let generation = self.0;
        vec![TestCase::new(
            "duplicate_header_generation",
            "Verify current first value, backend identity, and obsolete-value rejection",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    for value in ["one", "two", "other"] {
                        let port = generation.filter(|(first, _)| *first == value).map(|(_, port)| port);
                        if let Err(error) = HttpMatchTestSuite::check_duplicate_header_request(
                            &ctx,
                            "duplicate-header-dynamic.example.com",
                            "/echo/dynamic",
                            &[("x-mode", value)],
                            port,
                        )
                        .await
                        {
                            return TestResult::failed(start.elapsed(), error.to_string());
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(HeaderSuite(None))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(HeaderSuite(Some(("one", 30001))))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(HeaderSuite(Some(("two", 30002))))
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
    let response = client
        .delete("HTTPRoute", Some("edgion-test"), "duplicate-header-dynamic")
        .await?;
    ensure!(
        response.status().is_success() || response.status().as_u16() == 404,
        "duplicate header route cleanup failed: {}",
        response.status()
    );
    ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            // Static loaders may have preloaded either dynamic generation.
            cleanup(&ctx).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            ctx.apply_dir("updated/").await?;
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
    id: "HttpRouteDuplicateHeaders",
    conf_path: "HTTPRoute/Match/DynamicTest",
    run,
    phases: PHASES,
};
