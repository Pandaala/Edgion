use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites::GraphqlGuardDynamicSuite;
use anyhow::Result;
use std::{pin::Pin, time::Duration};
fn valid() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: false,
        strict: false,
        root_policy: false,
    })
}
fn invalid() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: true,
        strict: false,
        root_policy: false,
    })
}
fn strict() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: false,
        strict: true,
        root_policy: false,
    })
}
fn root_policy() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: false,
        strict: false,
        root_policy: true,
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
        name: "strict",
        factory: strict,
    },
    PhaseSuite {
        name: "root_policy",
        factory: root_policy,
    },
];
fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        ctx.assert_phase(SCENARIO, "initial").await?;
        for (dir, phase) in [
            ("to_invalid", "invalid"),
            ("to_valid", "recovered"),
            ("to_strict", "strict"),
            ("to_root_policy", "root_policy"),
        ] {
            ctx.apply_dir(&format!("updates/{dir}")).await?;
            ctx.wait_phase(SCENARIO, phase, Duration::from_secs(15)).await?;
        }
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "GraphqlGuardDynamic",
    conf_path: "EdgionPlugins/GraphqlGuard/DynamicTest",
    run,
    phases: PHASES,
};
