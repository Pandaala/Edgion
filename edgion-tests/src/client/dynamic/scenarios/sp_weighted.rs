// SessionPersistenceWeighted dynamic scenario (GEP-1619 traffic-split
// semantics).
//
// Initial: HTTPRoute weights=70/30 (spw-a/spw-b), Cookie mode. Verifies the
//          token-less split, pins a session to the weight-30 backend for 50
//          requests, verifies rule-scope isolation with the default session
//          name, and stores the spw-b cookie for later phases.
// Update1: weights -> 100/0. New sessions all go to spw-a; the established
//          spw-b session must NOT migrate (weight 0 does not move sessions).
// Update2: clears spw-b's EndpointSlice endpoints. The stale session falls
//          back to spw-a and the response carries a refreshed token.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::SpWeightedInitialTestSuite)
}

fn zero_weight() -> Box<dyn TestSuite> {
    Box::new(suites::SpWeightedZeroWeightTestSuite)
}

fn final_phase() -> Box<dyn TestSuite> {
    Box::new(suites::SpWeightedFinalTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "zero_weight",
        factory: zero_weight,
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
        ctx.apply_dir("updates/zero_weight_b").await?;
        ctx.wait_phase(scenario, "zero_weight", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/delete_b").await?;
        ctx.wait_phase(scenario, "final", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "SpWeighted",
    conf_path: "HTTPRoute/Backend/SessionPersistenceWeighted/DynamicTest",
    run,
    phases: PHASES,
};
