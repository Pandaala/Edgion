// RateLimitDynamic scenario.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::RateLimitDynamicInitialTestSuite)
}
fn update() -> Box<dyn TestSuite> {
    Box::new(suites::RateLimitDynamicUpdateTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "update",
        factory: update,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates/").await?;
        if let Err(e) = ctx.wait_resource_sync().await {
            eprintln!("resource_diff non-blocking: {}", e);
        }
        ctx.sleep(Duration::from_secs(3)).await;
        ctx.assert_phase(scenario, "update").await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "RateLimitDynamic",
    conf_path: "EdgionPlugins/RateLimit/DynamicTest",
    run,
    phases: PHASES,
};
