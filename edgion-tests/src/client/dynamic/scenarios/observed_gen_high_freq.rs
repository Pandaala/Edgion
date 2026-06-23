// ObservedGenHighFreq dynamic scenario.
//
// Applies 10 rapid Gateway spec changes (generation 1→11) and asserts that
// the controller's observedGeneration catches up to the final generation.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::HighFreqInitialTestSuite)
}

fn final_phase() -> Box<dyn TestSuite> {
    Box::new(suites::HighFreqFinalTestSuite)
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
        for i in 1..=10 {
            let dir = format!("updates/iter{}", i);
            ctx.apply_dir(&dir).await?;
        }
        ctx.wait_phase(scenario, "final", Duration::from_secs(30)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "ObservedGenHighFreq",
    conf_path: "Status/ObservedGeneration/HighFrequency/DynamicTest",
    run,
    phases: PHASES,
};
