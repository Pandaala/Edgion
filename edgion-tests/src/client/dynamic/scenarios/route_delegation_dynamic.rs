// RouteDelegationDynamic scenario.
//
// Initial: the child in edgion-deleg-dyn serves /api/v1/billing/pay through
// the parent's wildcard delegation.
// Update: only the child route changes (rule moves to /pay2); the controller
// must requeue the parent, re-flatten, and the gateway must hot-swap routing.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::DelegationDynamicInitialTestSuite)
}
fn update() -> Box<dyn TestSuite> {
    Box::new(suites::DelegationDynamicUpdateTestSuite)
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
    id: "RouteDelegationDynamic",
    conf_path: "HTTPRoute/Delegation/DynamicTest",
    run,
    phases: PHASES,
};
