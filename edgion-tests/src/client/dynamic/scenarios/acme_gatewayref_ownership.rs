use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites::{AcmeGatewayRefOwnershipPhase, AcmeGatewayRefOwnershipTestSuite};
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn selected_a() -> Box<dyn TestSuite> {
    Box::new(AcmeGatewayRefOwnershipTestSuite::new(
        AcmeGatewayRefOwnershipPhase::SelectedA,
    ))
}

fn selected_b() -> Box<dyn TestSuite> {
    Box::new(AcmeGatewayRefOwnershipTestSuite::new(
        AcmeGatewayRefOwnershipPhase::SelectedB,
    ))
}

fn removed() -> Box<dyn TestSuite> {
    Box::new(AcmeGatewayRefOwnershipTestSuite::new(
        AcmeGatewayRefOwnershipPhase::Removed,
    ))
}

fn expired() -> Box<dyn TestSuite> {
    Box::new(AcmeGatewayRefOwnershipTestSuite::new(
        AcmeGatewayRefOwnershipPhase::Expired,
    ))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "selected-a",
        factory: selected_a,
    },
    PhaseSuite {
        name: "selected-b",
        factory: selected_b,
    },
    PhaseSuite {
        name: "removed",
        factory: removed,
    },
    PhaseSuite {
        name: "expired",
        factory: expired,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.wait_phase(scenario, "selected-a", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/retarget").await?;
        ctx.wait_phase(scenario, "selected-b", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/remove").await?;
        ctx.wait_phase(scenario, "removed", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/expire").await?;
        ctx.wait_phase(scenario, "expired", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "AcmeGatewayRefOwnership",
    conf_path: "Gateway/AcmeGatewayRefOwnership/DynamicTest",
    run,
    phases: PHASES,
};
