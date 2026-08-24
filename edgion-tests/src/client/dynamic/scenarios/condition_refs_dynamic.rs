use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsDynamicInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsDynamicUpdatedTestSuite)
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsDynamicDeletedTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.apply_dir("initial/").await?;
        ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;

        ctx.apply_dir("updates/00_Secret_condition-refs-dynamic.yaml").await?;
        ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await?;

        ctx.delete_resources("delete/secret.txt").await?;
        ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await?;

        ctx.apply_dir("initial/00_Secret_condition-refs-dynamic.yaml").await?;
        ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "ConditionRefsDynamic",
    conf_path: "EdgionPlugins/ConditionRefsDynamic/DynamicTest",
    run,
    phases: PHASES,
};
