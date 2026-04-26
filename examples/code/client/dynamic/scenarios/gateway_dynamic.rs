// GatewayDynamic scenario — 从 run_integration.sh 的 run_dynamic_tests() 迁移而来。
//
// Phase initial: 验证基础 Gateway 配置生效。
// Transition: apply updates/ + delete delete/resources_to_delete.txt + wait sync + sleep 3s。
// Phase update: 验证热更新后新配置生效。

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial_suite() -> Box<dyn TestSuite> {
    Box::new(suites::InitialPhaseTestSuite)
}

fn update_suite() -> Box<dyn TestSuite> {
    Box::new(suites::UpdatePhaseTestSuite)
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
        ctx.delete_resources("delete/resources_to_delete.txt").await?;
        ctx.wait_phase(scenario, "update", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "GatewayDynamic",
    conf_path: "Gateway/DynamicTest",
    run,
    phases: PHASES,
};
