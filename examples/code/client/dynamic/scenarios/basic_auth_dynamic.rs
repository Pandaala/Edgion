// BasicAuthDynamic scenario.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::BasicAuthDynamicInitialTestSuite)
}
fn update() -> Box<dyn TestSuite> {
    Box::new(suites::BasicAuthDynamicUpdateTestSuite)
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
    id: "BasicAuthDynamic",
    conf_path: "EdgionPlugins/BasicAuth/DynamicTest",
    run,
    phases: PHASES,
};

// ============================================================
// Grouped variant scenario
// ============================================================

fn grouped_initial() -> Box<dyn TestSuite> {
    Box::new(suites::BasicAuthDynamicGroupedInitialTestSuite)
}
fn grouped_update() -> Box<dyn TestSuite> {
    Box::new(suites::BasicAuthDynamicGroupedUpdateTestSuite)
}

static GROUPED_PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: grouped_initial,
    },
    PhaseSuite {
        name: "update",
        factory: grouped_update,
    },
];

fn run_grouped(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = GROUPED_SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates_grouped/").await?;
        ctx.wait_phase(scenario, "update", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static GROUPED_SCENARIO: &Scenario = &Scenario {
    id: "BasicAuthDynamicGrouped",
    // Same root as BasicAuthDynamic; initial_grouped/ is applied by start_all_with_conf.sh,
    // and updates_grouped/ is applied via ctx.apply_dir("updates_grouped/") below.
    conf_path: "EdgionPlugins/BasicAuth/DynamicTest",
    run: run_grouped,
    phases: GROUPED_PHASES,
};
