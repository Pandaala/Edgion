// GcParamsFlap dynamic scenario (controller-07).
//
// Exercises the EdgionGatewayConfig -> GatewayClass requeue that keeps the GatewayClass
// ResolvedRefs/Accepted conditions fresh:
//   missing:  GatewayClass references params-flap-egc, which does not exist
//             -> ResolvedRefs=False, Accepted=False
//   present:  the EdgionGatewayConfig is created (apply updates/to_present)
//             -> requeue flips the GatewayClass to ResolvedRefs=True, Accepted=True
//   gone:     the EdgionGatewayConfig is deleted again
//             -> on_delete requeue flips it back to ResolvedRefs=False, Accepted=False

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn missing() -> Box<dyn TestSuite> {
    Box::new(suites::GcParamsFlapMissingTestSuite)
}

fn present() -> Box<dyn TestSuite> {
    Box::new(suites::GcParamsFlapPresentTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "missing",
        factory: missing,
    },
    PhaseSuite {
        name: "present",
        factory: present,
    },
    PhaseSuite {
        name: "gone",
        factory: missing,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "missing").await?;
        ctx.apply_dir("updates/to_present").await?;
        ctx.wait_phase(scenario, "present", Duration::from_secs(15)).await?;
        ctx.delete_resources("delete/egc.txt").await?;
        ctx.wait_phase(scenario, "gone", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "GcParamsFlap",
    conf_path: "Status/GatewayClassParamsRef/RefFlap/DynamicTest",
    run,
    phases: PHASES,
};
