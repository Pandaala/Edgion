use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn invalid_suite() -> Box<dyn TestSuite> {
    Box::new(suites::JsonSchemaValidationDynamicInvalidTestSuite)
}

fn valid_v1_suite() -> Box<dyn TestSuite> {
    Box::new(suites::JsonSchemaValidationDynamicV1TestSuite)
}

fn valid_v2_suite() -> Box<dyn TestSuite> {
    Box::new(suites::JsonSchemaValidationDynamicV2TestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "invalid-initial",
        factory: invalid_suite,
    },
    PhaseSuite {
        name: "valid-v1",
        factory: valid_v1_suite,
    },
    PhaseSuite {
        name: "invalid-reload",
        factory: invalid_suite,
    },
    PhaseSuite {
        name: "valid-v2",
        factory: valid_v2_suite,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "invalid-initial").await?;
        ctx.apply_dir("updates/to_valid_v1").await?;
        ctx.wait_phase(scenario, "valid-v1", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_invalid").await?;
        ctx.wait_phase(scenario, "invalid-reload", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_valid_v2").await?;
        ctx.wait_phase(scenario, "valid-v2", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "JsonSchemaValidationDynamic",
    conf_path: "EdgionPlugins/JsonSchemaValidation/DynamicTest",
    run,
    phases: PHASES,
};
