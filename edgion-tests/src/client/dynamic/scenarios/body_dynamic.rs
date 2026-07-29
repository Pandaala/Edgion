use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial_suite() -> Box<dyn TestSuite> {
    Box::new(suites::BodyDynamicInitialTestSuite)
}

fn update_suite() -> Box<dyn TestSuite> {
    Box::new(suites::BodyDynamicUpdateTestSuite)
}

fn capped_suite() -> Box<dyn TestSuite> {
    Box::new(suites::BodyDynamicCappedTestSuite)
}

fn rejected_suite() -> Box<dyn TestSuite> {
    Box::new(suites::BodyDynamicRejectedTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial_suite,
    },
    PhaseSuite {
        name: "capped",
        factory: capped_suite,
    },
    PhaseSuite {
        name: "restored",
        factory: update_suite,
    },
    PhaseSuite {
        name: "rejected",
        factory: rejected_suite,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates/to_capped").await?;
        ctx.wait_phase(scenario, "capped", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_restored").await?;
        ctx.wait_phase(scenario, "restored", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_invalid").await?;
        ctx.wait_phase(scenario, "rejected", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "BodyDynamic",
    conf_path: "EdgionPlugins/BodyDynamic/DynamicTest",
    run,
    phases: PHASES,
};
