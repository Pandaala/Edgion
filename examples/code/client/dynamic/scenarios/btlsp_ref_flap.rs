// BtlspRefFlap dynamic scenario.
//
// Applies a BackendTLSPolicy that switches its CA ConfigMap reference:
//   initial:  references existing ca-good  → Accepted=True, ResolvedRefs=True
//   invalid:  references missing ca-missing → Accepted=False, ResolvedRefs=False
//   valid:    restored to ca-good           → Accepted=True, ResolvedRefs=True

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::BtlspFlapInitialTestSuite)
}

fn invalid() -> Box<dyn TestSuite> {
    Box::new(suites::BtlspFlapInvalidTestSuite)
}

fn valid() -> Box<dyn TestSuite> {
    Box::new(suites::BtlspFlapValidTestSuite)
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
    id: "BtlspRefFlap",
    conf_path: "Status/BackendTLSPolicy/RefFlap/DynamicTest",
    run,
    phases: PHASES,
};
