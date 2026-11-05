// GrpcRouteDelegationDynamic scenario.
//
// Initial: the child in edgion-grpc-deleg-dyn narrows the parent's wildcard
// delegation of test.TestService to SayHello.
// Update: only the child route changes (the narrowed method moves to
// StreamNumbers); the controller republishes only the child (its delegation
// keys are unchanged) and the gateway must rebuild the child's subtree slot
// and hot-swap routing without touching the parent.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::GrpcDelegationDynamicInitialTestSuite)
}
fn update() -> Box<dyn TestSuite> {
    Box::new(suites::GrpcDelegationDynamicUpdateTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "update",
        factory: update,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates/").await?;
        ctx.wait_phase(scenario, "update", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "GrpcRouteDelegationDynamic",
    conf_path: "GRPCRoute/Delegation/DynamicTest",
    run,
    phases: PHASES,
};
