use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites::GraphqlSafelistDynamicSuite;
use anyhow::Result;
use std::{pin::Pin, time::Duration};
fn valid() -> Box<dyn TestSuite> {
    Box::new(GraphqlSafelistDynamicSuite {
        invalid: false,
        revoked: false,
    })
}
fn invalid() -> Box<dyn TestSuite> {
    Box::new(GraphqlSafelistDynamicSuite {
        invalid: true,
        revoked: false,
    })
}
fn revoked() -> Box<dyn TestSuite> {
    Box::new(GraphqlSafelistDynamicSuite {
        invalid: false,
        revoked: true,
    })
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: valid,
    },
    PhaseSuite {
        name: "invalid",
        factory: invalid,
    },
    PhaseSuite {
        name: "recovered",
        factory: valid,
    },
    PhaseSuite {
        name: "revoked",
        factory: revoked,
    },
];
fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        ctx.assert_phase(SCENARIO, "initial").await?;
        for (dir, phase) in [
            ("to_invalid", "invalid"),
            ("to_valid", "recovered"),
            ("to_revoked", "revoked"),
        ] {
            ctx.apply_dir(&format!("updates/{dir}")).await?;
            ctx.wait_phase(SCENARIO, phase, Duration::from_secs(15)).await?;
        }
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "GraphqlSafelistDynamic",
    conf_path: "EdgionPlugins/GraphqlSafelist/DynamicTest",
    run,
    phases: PHASES,
};
