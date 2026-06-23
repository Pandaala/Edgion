// GatewayDynamic scenario.
//
// Phase initial: verify the baseline Gateway configuration takes effect.
// Transition: apply updates/, delete resources_to_delete.txt, wait up to 15s for sync.
// Phase update: verify the new configuration takes effect after hot reload.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial_suite() -> Box<dyn TestSuite> {
    Box::new(suites::InitialPhaseTestSuite)
}

fn update_suite() -> Box<dyn TestSuite> {
    Box::new(suites::UpdatePhaseTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial_suite,
    },
    PhaseSuite {
        name: "update",
        factory: update_suite,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates/").await?;
        ctx.delete_resources("delete/resources_to_delete.txt").await?;
        ctx.wait_phase(scenario, "update", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "GatewayDynamic",
    conf_path: "Gateway/DynamicTest",
    run,
    phases: PHASES,
};
