// SpBackendChange dynamic scenario.
//
// Initial: HTTPRoute weights=100/0/0, SessionPersistence Cookie mode.
//          Pins SP cookie to backend-a, stores cookie for final phase.
// Update:  Clears backend-a EndpointSlice endpoints + shifts weights to 0/50/50.
// Final:   Same cookie must route to backend-b or backend-c (not 503, not backend-a).

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::SpChangeInitialTestSuite)
}

fn final_phase() -> Box<dyn TestSuite> {
    Box::new(suites::SpChangeFinalTestSuite)
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
        ctx.apply_dir("updates/delete_a").await?;
        ctx.wait_phase(scenario, "final", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "SpBackendChange",
    conf_path: "HTTPRoute/Backend/SessionPersistenceBackendChange/DynamicTest",
    run,
    phases: PHASES,
};
