use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn form_to_json_suite() -> Box<dyn TestSuite> {
    Box::new(suites::FormJsonTransformDynamicFormTestSuite)
}

fn json_to_form_suite() -> Box<dyn TestSuite> {
    Box::new(suites::FormJsonTransformDynamicJsonTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "form-to-json",
        factory: form_to_json_suite,
    },
    PhaseSuite {
        name: "json-to-form",
        factory: json_to_form_suite,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "form-to-json").await?;
        ctx.apply_dir("updates/to_json_to_form").await?;
        ctx.wait_phase(scenario, "json-to-form", Duration::from_secs(15))
            .await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "FormJsonTransformDynamic",
    conf_path: "EdgionPlugins/FormJsonTransform/DynamicTest",
    run,
    phases: PHASES,
};
