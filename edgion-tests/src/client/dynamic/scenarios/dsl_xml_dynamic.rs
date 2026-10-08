use crate::dynamic::{PhaseSuite, Scenario, ScenarioCtx};
use crate::framework::TestSuite;
use crate::suites::DslXmlDynamicTestSuite;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(DslXmlDynamicTestSuite::new("initial"))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(DslXmlDynamicTestSuite::new("updated"))
}
fn capped() -> Box<dyn TestSuite> {
    Box::new(DslXmlDynamicTestSuite::new("capped"))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "capped",
        factory: capped,
    },
    PhaseSuite {
        name: "restored",
        factory: updated,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let result = async {
            ctx.apply_dir("updates/to_restored").await?;
            ctx.apply_dir("initial").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(15)).await?;
            for (directory, phase) in [
                ("updates/to_updated", "updated"),
                ("updates/to_capped", "capped"),
                ("updates/to_restored", "restored"),
            ] {
                ctx.apply_dir(directory).await?;
                ctx.wait_phase(SCENARIO, phase, Duration::from_secs(15)).await?;
            }
            Ok(())
        }
        .await;
        let cleanup = async {
            ctx.apply_dir("updates/to_restored").await?;
            ctx.apply_dir("initial").await?;
            // Cleanup verifies traffic without repeating the K8s phase barrier.
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    let mut passed = true;
                    for case in DslXmlDynamicTestSuite::new("initial").test_cases() {
                        passed &= case.run(&ctx.test_ctx).await.passed;
                    }
                    if passed {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            })
            .await
            .map_err(|_| anyhow::anyhow!("XML dynamic cleanup traffic did not converge"))
        }
        .await;
        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup_error)) => {
                Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup_error:#}"))
            }
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "DslXmlDynamic",
    conf_path: "EdgionPlugins/DslBody/DynamicTest",
    run,
    phases: PHASES,
};
