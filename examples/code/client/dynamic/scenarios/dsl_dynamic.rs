// DslDynamic scenario — 复用现有 DslDynamicInitialTestSuite / DslDynamicUpdateTestSuite。
//
// Initial: X-Version=v1 → 200, X-Version=v2 → 403
// Transition: apply updates/ (DSL source 切换到 v2 放行)
// Update: X-Version=v2 → 200, X-Version=v1 → 403

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial_suite() -> Box<dyn TestSuite> {
    Box::new(suites::DslDynamicInitialTestSuite)
}

fn update_suite() -> Box<dyn TestSuite> {
    Box::new(suites::DslDynamicUpdateTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial_suite,
    },
    PhaseSuite {
        name: "update",
        factory: update_suite,
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
    id: "DslDynamic",
    conf_path: "EdgionPlugins/DslDynamic/DynamicTest",
    run,
    phases: PHASES,
};
