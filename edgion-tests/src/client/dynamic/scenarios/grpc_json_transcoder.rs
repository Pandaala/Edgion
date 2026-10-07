use crate::dynamic::{
    scenario::{PhaseSuite, Scenario, ScenarioRunFuture},
    ScenarioCtx,
};
use crate::framework::TestSuite;
use crate::suites::GrpcJsonTranscoderDynamicSuite;
use std::time::Duration;
fn valid() -> Box<dyn TestSuite> {
    Box::new(GrpcJsonTranscoderDynamicSuite { valid: true })
}
fn invalid() -> Box<dyn TestSuite> {
    Box::new(GrpcJsonTranscoderDynamicSuite { valid: false })
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "valid",
        factory: valid,
    },
    PhaseSuite {
        name: "invalid",
        factory: invalid,
    },
    PhaseSuite {
        name: "deleted",
        factory: invalid,
    },
];
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        ctx.assert_phase(SCENARIO, "valid").await?;
        ctx.apply_dir("updates/invalid").await?;
        ctx.wait_phase(SCENARIO, "invalid", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/recovery").await?;
        ctx.wait_phase(SCENARIO, "valid", Duration::from_secs(15)).await?;
        ctx.delete_resources("delete/resources_to_delete.txt").await?;
        ctx.wait_phase(SCENARIO, "deleted", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/recovery").await?;
        ctx.wait_phase(SCENARIO, "valid", Duration::from_secs(15)).await?;
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "GrpcJsonTranscoderDynamic",
    conf_path: "EdgionPlugins/GrpcJsonTranscoder/DynamicTest",
    run,
    phases: PHASES,
};
