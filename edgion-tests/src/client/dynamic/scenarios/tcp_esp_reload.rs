// TcpEspReload dynamic scenario.
//
// Hot-reloads the EdgionStreamPlugins allow-list on a single TCPRoute listener:
//   initial : ESP allows the Kubernetes client -> TCP echo succeeds
//   deny    : allow-list removed      -> connection denied
//   restored: allow-list restored     -> TCP echo succeeds again

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn allow() -> Box<dyn TestSuite> {
    Box::new(suites::TcpEspReloadAllowTestSuite)
}

fn deny() -> Box<dyn TestSuite> {
    Box::new(suites::TcpEspReloadDenyTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: allow,
    },
    PhaseSuite {
        name: "deny",
        factory: deny,
    },
    PhaseSuite {
        name: "restored",
        factory: allow,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates/to_deny").await?;
        ctx.wait_phase(scenario, "deny", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_allow").await?;
        ctx.wait_phase(scenario, "restored", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "TcpEspReload",
    conf_path: "TCPRoute/StreamPlugins/DynamicTest",
    run,
    phases: PHASES,
};
