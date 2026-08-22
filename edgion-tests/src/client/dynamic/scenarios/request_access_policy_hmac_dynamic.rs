use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::RequestAccessPolicyHmacDynamicInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::RequestAccessPolicyHmacDynamicUpdatedTestSuite)
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(suites::RequestAccessPolicyHmacDynamicDeletedTestSuite)
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

        let rotation_result = async {
            ctx.apply_dir("updates/00_Secret_gac-hmac-dynamic.yaml").await?;
            ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await
        }
        .await;

        let delete_result = async {
            ctx.delete_resources("delete/secret.txt").await?;
            ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await
        }
        .await;

        let restore_result = async {
            ctx.apply_dir("initial/00_Secret_gac-hmac-dynamic.yaml").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await
        }
        .await;
        restore_result?;
        rotation_result?;
        delete_result
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "RequestAccessPolicyHmacDynamic",
    conf_path: "EdgionPlugins/RequestAccessPolicy/DynamicTest",
    run,
    phases: PHASES,
};
