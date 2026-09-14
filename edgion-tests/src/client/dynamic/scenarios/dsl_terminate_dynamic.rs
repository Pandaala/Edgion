//! DSL explicit-termination publication scenario.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial_suite() -> Box<dyn TestSuite> {
    Box::new(suites::DslTerminateDynamicInitialTestSuite)
}

fn updated_suite() -> Box<dyn TestSuite> {
    Box::new(suites::DslTerminateDynamicUpdatedTestSuite)
}

fn restored_suite() -> Box<dyn TestSuite> {
    Box::new(suites::DslTerminateDynamicRestoredTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial_suite,
    },
    PhaseSuite {
        name: "updated",
        factory: updated_suite,
    },
    PhaseSuite {
        name: "restored",
        factory: restored_suite,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        let result = async {
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates/").await?;
            ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates_grouped/").await?;
            ctx.wait_phase(scenario, "restored", Duration::from_secs(15)).await
        }
        .await;

        let cleanup = async {
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await
        }
        .await;

        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup_error)) => {
                Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup_error:#}"))
            }
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "DslTerminateDynamic",
    conf_path: "EdgionPlugins/DslTerminateDynamic/DynamicTest",
    run,
    phases: PHASES,
};
