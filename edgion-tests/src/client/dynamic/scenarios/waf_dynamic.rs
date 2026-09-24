//! Native WAF generation publication and last-known-good scenario.

use std::time::Duration;

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicUpdatedTestSuite)
}

fn recovered() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicRecoveredTestSuite)
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicDeletedTestSuite)
}

fn configured_prefix() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicConfiguredPrefixTestSuite)
}

fn detection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicDetectionTestSuite)
}

fn injection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicInjectionTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "configured-prefix",
        factory: configured_prefix,
    },
    PhaseSuite {
        name: "detection",
        factory: detection,
    },
    PhaseSuite {
        name: "injection",
        factory: injection,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "invalid-retained",
        factory: updated,
    },
    PhaseSuite {
        name: "recovered",
        factory: recovered,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates/to_v2").await?;
        ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_invalid").await?;
        ctx.wait_phase(scenario, "invalid-retained", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_prefix_1m").await?;
        ctx.wait_phase(scenario, "configured-prefix", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_detection").await?;
        ctx.wait_phase(scenario, "detection", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_injection").await?;
        ctx.wait_phase(scenario, "injection", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_recovered").await?;
        ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await?;
        ctx.delete_resources("delete/resources.txt").await?;
        ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_recovered").await?;
        ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "WafDynamic",
    conf_path: "EdgionPlugins/Waf/DynamicTest",
    run,
    phases: PHASES,
};
