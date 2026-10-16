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
        let result: Result<()> = async {
            let scenario = SCENARIO;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates/").await?;
            ctx.wait_phase(scenario, "update", Duration::from_secs(15)).await?;
            Ok(())
        }
        .await;
        let restored = async {
            ctx.apply_dir("initial/EdgionPlugins_basic-auth-dynamic.yaml").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(15)).await
        }
        .await;
        match (result, restored) {
            (Ok(()), restored) => restored,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(restored)) => Err(anyhow::anyhow!("{error:#}; fixture restoration failed: {restored:#}")),
        }
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
        let result: Result<()> = async {
            let scenario = GROUPED_SCENARIO;
            ctx.apply_dir("initial_grouped/").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates_grouped/").await?;
            ctx.wait_phase(scenario, "update", Duration::from_secs(15)).await?;
            Ok(())
        }
        .await;
        let restored = async {
            ctx.apply_dir("initial_grouped/EdgionPlugins_basic-auth-dynamic-grouped.yaml")
                .await?;
            ctx.wait_phase(GROUPED_SCENARIO, "initial", Duration::from_secs(15))
                .await
        }
        .await;
        match (result, restored) {
            (Ok(()), restored) => restored,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(restored)) => Err(anyhow::anyhow!("{error:#}; fixture restoration failed: {restored:#}")),
        }
    })
}

pub static GROUPED_SCENARIO: &Scenario = &Scenario {
    id: "BasicAuthDynamicGrouped",
    // Both variants own establishment and restoration within this shared fixture root.
    conf_path: "EdgionPlugins/BasicAuth/DynamicTest",
    run: run_grouped,
    phases: GROUPED_PHASES,
};
