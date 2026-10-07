use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites::GraphqlRateLimitDynamicSuite;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn invalid() -> Box<dyn TestSuite> {
    Box::new(GraphqlRateLimitDynamicSuite {
        valid: false,
        expected_remaining: 0,
    })
}
fn valid_v1() -> Box<dyn TestSuite> {
    Box::new(GraphqlRateLimitDynamicSuite {
        valid: true,
        expected_remaining: 988,
    })
}
fn valid_v2() -> Box<dyn TestSuite> {
    Box::new(GraphqlRateLimitDynamicSuite {
        valid: true,
        expected_remaining: 984,
    })
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "invalid-initial",
        factory: invalid,
    },
    PhaseSuite {
        name: "valid-v1",
        factory: valid_v1,
    },
    PhaseSuite {
        name: "invalid-reload",
        factory: invalid,
    },
    PhaseSuite {
        name: "valid-v2",
        factory: valid_v2,
    },
    PhaseSuite {
        name: "invalid-leaf-reload",
        factory: invalid,
    },
    PhaseSuite {
        name: "recovered-v2",
        factory: valid_v2,
    },
];
fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        ctx.assert_phase(SCENARIO, "invalid-initial").await?;
        ctx.apply_dir("updates/to_valid_v1").await?;
        ctx.wait_phase(SCENARIO, "valid-v1", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_invalid").await?;
        ctx.wait_phase(SCENARIO, "invalid-reload", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_valid_v2").await?;
        ctx.wait_phase(SCENARIO, "valid-v2", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_invalid_leaf").await?;
        ctx.wait_phase(SCENARIO, "invalid-leaf-reload", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_valid_v2").await?;
        ctx.wait_phase(SCENARIO, "recovered-v2", Duration::from_secs(15))
            .await?;
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "GraphqlRateLimitDynamic",
    conf_path: "EdgionPlugins/GraphqlRateLimit/DynamicTest",
    run,
    phases: PHASES,
};
