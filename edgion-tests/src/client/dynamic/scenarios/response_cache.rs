//! One shared-topology cache publication lifecycle.
use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites::edgion_plugins::response_cache::ResponseCacheDynamicSuite;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(ResponseCacheDynamicSuite { phase: "initial" })
}
fn disabled() -> Box<dyn TestSuite> {
    Box::new(ResponseCacheDynamicSuite { phase: "disabled" })
}
fn reenabled() -> Box<dyn TestSuite> {
    Box::new(ResponseCacheDynamicSuite { phase: "reenabled" })
}
fn body_filter() -> Box<dyn TestSuite> {
    Box::new(ResponseCacheDynamicSuite { phase: "body-filter" })
}
fn target() -> Box<dyn TestSuite> {
    Box::new(ResponseCacheDynamicSuite { phase: "target" })
}
fn deleted() -> Box<dyn TestSuite> {
    Box::new(ResponseCacheDynamicSuite { phase: "deleted" })
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "disabled",
        factory: disabled,
    },
    PhaseSuite {
        name: "reenabled",
        factory: reenabled,
    },
    PhaseSuite {
        name: "body-filter",
        factory: body_filter,
    },
    PhaseSuite {
        name: "target",
        factory: target,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
];
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        ctx.assert_phase(SCENARIO, "initial").await?;
        for (directory, phase) in [
            ("disable", "disabled"),
            ("reenable", "reenabled"),
            ("body-filter", "body-filter"),
        ] {
            ctx.apply_dir(&format!("updates/{directory}")).await?;
            ctx.assert_phase(SCENARIO, phase).await?;
        }
        // Restore the safe certificate and warm it before replacing its origin.
        ctx.apply_dir("updates/reenable").await?;
        ctx.assert_phase(SCENARIO, "reenabled").await?;
        ctx.apply_dir("updates/target").await?;
        ctx.assert_phase(SCENARIO, "target").await?;
        ctx.delete_resources("delete/resources_to_delete.txt").await?;
        ctx.wait_phase(SCENARIO, "deleted", Duration::from_secs(20)).await?;
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "ResponseCacheDynamic",
    conf_path: "EdgionPlugins/ResponseCache/DynamicTest",
    run,
    phases: PHASES,
};
