use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites::RestToGraphqlDynamicSuite;
use anyhow::Result;
use std::{pin::Pin, time::Duration};

fn initial() -> Box<dyn TestSuite> {
    Box::new(RestToGraphqlDynamicSuite {
        invalid: false,
        recovered: false,
    })
}
fn invalid() -> Box<dyn TestSuite> {
    Box::new(RestToGraphqlDynamicSuite {
        invalid: true,
        recovered: false,
    })
}
fn recovered() -> Box<dyn TestSuite> {
    Box::new(RestToGraphqlDynamicSuite {
        invalid: false,
        recovered: true,
    })
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "invalid",
        factory: invalid,
    },
    PhaseSuite {
        name: "recovered",
        factory: recovered,
    },
];
fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        ctx.assert_phase(SCENARIO, "initial").await?;
        for (directory, phase) in [("to_invalid", "invalid"), ("to_valid", "recovered")] {
            ctx.apply_dir(&format!("updates/{directory}")).await?;
            ctx.wait_phase(SCENARIO, phase, Duration::from_secs(15)).await?;
        }
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "RestToGraphqlDynamic",
    conf_path: "EdgionPlugins/RestToGraphql/DynamicTest",
    run,
    phases: PHASES,
};
