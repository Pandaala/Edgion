use std::time::Duration;

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::CredentialInjectorDynamicInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::CredentialInjectorDynamicUpdatedTestSuite)
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(suites::CredentialInjectorDynamicDeletedTestSuite)
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

        let update_result = async {
            ctx.apply_dir("updates/00_Secret_credential-injector-dynamic.yaml")
                .await?;
            ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await
        }
        .await;

        let delete_result = async {
            ctx.delete_resources("delete/secret.txt").await?;
            ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await
        }
        .await;

        let restore_result = async {
            ctx.apply_dir("initial/00_Secret_credential-injector-dynamic.yaml")
                .await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await
        }
        .await;

        restore_result?;
        update_result?;
        delete_result
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "CredentialInjectorDynamic",
    conf_path: "EdgionPlugins/CredentialInjector/DynamicTest",
    run,
    phases: PHASES,
};
