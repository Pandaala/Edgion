// PmdRefFlap dynamic scenario.
//
// Applies an EdgionPlugins resource that switches its serviceRegionRouteRef:
//   initial:  references existing PMD pmd-flap-good  → Accepted=True
//   invalid:  references missing pmd-flap-missing    → Accepted=False (reason: Invalid)
//   valid:    restored to pmd-flap-good               → Accepted=True

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::PmdFlapInitialTestSuite)
}

fn invalid() -> Box<dyn TestSuite> {
    Box::new(suites::PmdFlapInvalidTestSuite)
}

fn valid() -> Box<dyn TestSuite> {
    Box::new(suites::PmdFlapValidTestSuite)
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
    conf_path: "EdgionPlugins/PluginMetaDataStatus/RefFlap/DynamicTest",
    run,
    phases: PHASES,
};
