// PmdRefFlap dynamic scenario.
//
// Applies an EdgionPlugins resource that switches its overrideRef (RegionRouteOverride EdgionData):
//   initial:  overrideRef → existing EdgionData data-flap-good   → Accepted=True, ResolvedRefs=True
//   invalid:  overrideRef → missing EdgionData data-flap-missing  → ResolvedRefs=False (InvalidEdgionDataRef)
//   valid:    restored to data-flap-good                          → Accepted=True, ResolvedRefs=True

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::EdgionDataFlapInitialTestSuite)
}

fn invalid() -> Box<dyn TestSuite> {
    Box::new(suites::EdgionDataFlapInvalidTestSuite)
}

fn valid() -> Box<dyn TestSuite> {
    Box::new(suites::EdgionDataFlapValidTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "invalid",
        factory: invalid,
    },
    PhaseSuite {
        name: "valid",
        factory: valid,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates/to_invalid").await?;
        ctx.wait_phase(scenario, "invalid", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_valid").await?;
        ctx.wait_phase(scenario, "valid", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "PmdRefFlap",
    conf_path: "EdgionPlugins/EdgionDataStatus/RefFlap/DynamicTest",
    run,
    phases: PHASES,
};
