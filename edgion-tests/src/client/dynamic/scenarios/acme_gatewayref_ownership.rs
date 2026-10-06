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

fn invalid_account() -> Box<dyn TestSuite> {
    Box::new(AcmeGatewayRefOwnershipTestSuite::new(
        AcmeGatewayRefOwnershipPhase::InvalidAccount,
    ))
}

fn invalid_eab() -> Box<dyn TestSuite> {
    Box::new(AcmeGatewayRefOwnershipTestSuite::new(
        AcmeGatewayRefOwnershipPhase::InvalidEab,
    ))
}

fn restored_references() -> Box<dyn TestSuite> {
    Box::new(AcmeGatewayRefOwnershipTestSuite::new(
        AcmeGatewayRefOwnershipPhase::RestoredReferences,
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
        name: "invalid-account",
        factory: invalid_account,
    },
    PhaseSuite {
        name: "invalid-eab",
        factory: invalid_eab,
    },
    PhaseSuite {
        name: "restored-references",
        factory: restored_references,
    },
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
        ctx.apply_dir("updates/invalid-account").await?;
        ctx.wait_phase(scenario, "invalid-account", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("initial").await?;
        ctx.wait_phase(scenario, "restored-references", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/invalid-eab").await?;
        ctx.wait_phase(scenario, "invalid-eab", Duration::from_secs(15)).await?;
        ctx.apply_dir("initial").await?;
        ctx.wait_phase(scenario, "restored-references", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/retarget").await?;
        ctx.wait_phase(scenario, "selected-b", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/remove").await?;
        ctx.wait_phase(scenario, "removed", Duration::from_secs(15)).await?;
        // Admin PUT preserves status, so expiry cannot be injected in-place.
        // Delete then create so the expired challenge is the object's first status.
        ctx.delete_resources("delete/edgionacme.txt").await?;
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
