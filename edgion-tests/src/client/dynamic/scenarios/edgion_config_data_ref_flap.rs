// PmdRefFlap dynamic scenario.
//
// Applies an EdgionPlugins resource that switches its clusterRegionRef (ClusterRegionSetting EdgionConfigData):
//   initial:  clusterRegionRef → existing EdgionConfigData data-flap-good   → Accepted=True, ResolvedRefs=True
//   invalid:  clusterRegionRef → missing EdgionConfigData data-flap-missing  → ResolvedRefs=False (InvalidEdgionConfigDataRef)
//   valid:    restored to data-flap-good                          → Accepted=True, ResolvedRefs=True

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::EdgionConfigDataFlapInitialTestSuite)
}

fn invalid() -> Box<dyn TestSuite> {
    Box::new(suites::EdgionConfigDataFlapInvalidTestSuite)
}

fn valid() -> Box<dyn TestSuite> {
    Box::new(suites::EdgionConfigDataFlapValidTestSuite)
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
    conf_path: "EdgionPlugins/EdgionConfigDataStatus/RefFlap/DynamicTest",
    run,
    phases: PHASES,
};
