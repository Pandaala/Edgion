use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites::GraphqlOperationRouterDynamicSuite;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;
fn users() -> Box<dyn TestSuite> {
    Box::new(GraphqlOperationRouterDynamicSuite {
        port: Some(30001),
        remote_allowed: false,
    })
}
fn orders() -> Box<dyn TestSuite> {
    Box::new(GraphqlOperationRouterDynamicSuite {
        port: Some(30002),
        remote_allowed: false,
    })
}
fn invalid() -> Box<dyn TestSuite> {
    Box::new(GraphqlOperationRouterDynamicSuite {
        port: None,
        remote_allowed: false,
    })
}
fn granted() -> Box<dyn TestSuite> {
    Box::new(GraphqlOperationRouterDynamicSuite {
        port: Some(30002),
        remote_allowed: true,
    })
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: users,
    },
    PhaseSuite {
        name: "invalid-unselected",
        factory: invalid,
    },
    PhaseSuite {
        name: "restored",
        factory: users,
    },
    PhaseSuite {
        name: "backend-removed",
        factory: invalid,
    },
    PhaseSuite {
        name: "backend-restored",
        factory: users,
    },
    PhaseSuite {
        name: "switched",
        factory: orders,
    },
    PhaseSuite {
        name: "grant-added",
        factory: granted,
    },
    PhaseSuite {
        name: "grant-removed",
        factory: orders,
    },
];
fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        ctx.assert_phase(SCENARIO, "initial").await?;
        for (directory, phase) in [
            ("to_invalid", "invalid-unselected"),
            ("to_valid", "restored"),
            ("to_remove", "backend-removed"),
            ("to_restore", "backend-restored"),
            ("to_switch", "switched"),
        ] {
            ctx.apply_dir(&format!("updates/{directory}")).await?;
            ctx.wait_phase(SCENARIO, phase, Duration::from_secs(15)).await?;
        }
        ctx.apply_dir("updates/to_grant").await?;
        ctx.wait_phase(SCENARIO, "grant-added", Duration::from_secs(15)).await?;
        ctx.delete_resources("delete/grant.txt").await?;
        ctx.wait_phase(SCENARIO, "grant-removed", Duration::from_secs(15))
            .await?;
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "GraphqlOperationRouterDynamic",
    conf_path: "EdgionPlugins/GraphqlOperationRouter/DynamicTest",
    run,
    phases: PHASES,
};
