// CanaryHotSwap dynamic scenario.
//
// Initial: Canary plugin activeProfile=stable, Selector.active=stable → routes to port 30001.
// Update:  Selector.active flipped to "canary" (only the EdgionConfigData changes, no
//          EdgionPlugins/HTTPRoute edit) → routes to port 30002.
// This is the core "dynamic switch" claim of the Canary plugin design.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::CanaryHotSwapInitialTestSuite)
}

fn final_phase() -> Box<dyn TestSuite> {
    Box::new(suites::CanaryHotSwapFinalTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "final",
        factory: final_phase,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates/flip_to_v2").await?;
        ctx.wait_phase(scenario, "final", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "CanaryHotSwap",
    conf_path: "EdgionPlugins/Canary/HotSwap/DynamicTest",
    run,
    phases: PHASES,
};
