use std::time::Duration;

use anyhow::Result;

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;

fn v1_suite() -> Box<dyn TestSuite> {
    Box::new(suites::A2aGatewayDynamicV1TestSuite)
}

fn v2_suite() -> Box<dyn TestSuite> {
    Box::new(suites::A2aGatewayDynamicV2TestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "v1",
        factory: v1_suite,
    },
    PhaseSuite {
        name: "v2",
        factory: v2_suite,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        ctx.assert_phase(SCENARIO, "v1").await?;
        ctx.apply_dir("updates/to_v2").await?;
        ctx.wait_phase(SCENARIO, "v2", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "A2aGatewayDynamic",
    conf_path: "EdgionPlugins/A2aGateway/DynamicTest",
    run,
    phases: PHASES,
};
