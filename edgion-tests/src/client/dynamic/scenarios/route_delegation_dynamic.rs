//! Child-only updates rebuild the delegated subtree without touching the
//! parent; parent-only narrowing changes the parent level's scope, which
//! intersects the child's absolute paths (including its regex rule).

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::{Duration, Instant};

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::DelegationDynamicInitialTestSuite)
}
fn update() -> Box<dyn TestSuite> {
    Box::new(suites::DelegationDynamicUpdateTestSuite)
}
fn narrowed() -> Box<dyn TestSuite> {
    Box::new(ScopeSuite { absent: false })
}
fn absent() -> Box<dyn TestSuite> {
    Box::new(ScopeSuite { absent: true })
}

struct ScopeSuite {
    absent: bool,
}

impl TestSuite for ScopeSuite {
    fn name(&self) -> &str {
        "Delegation parent scope publication"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let absent = self.absent;
        vec![TestCase::new(
            "parent_scope_generation",
            "Verify parent-only narrowing or complete route withdrawal",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    for (path, expected) in [
                        ("/api/v1/billing/pay2", if absent { 404 } else { 200 }),
                        ("/api/v1/billing/pay", 404),
                        ("/api/v1/billing/guard", 404),
                        ("/api/v1/billing/guard2", 404),
                        ("/api/v1/billingguard2", 404),
                    ] {
                        let response = ctx
                            .http_client
                            .get(format!("{}{path}", ctx.http_url()))
                            .header("Host", "delegation-dyn.example.com")
                            .send()
                            .await;
                        match response {
                            Ok(response) if response.status().as_u16() == expected => {}
                            result => {
                                return TestResult::failed(
                                    Instant::now().saturating_duration_since(start),
                                    format!("{path}: expected {expected}, got {result:?}"),
                                )
                            }
                        }
                    }
                    TestResult::passed(Instant::now().saturating_duration_since(start))
                })
            },
        )]
    }
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "update",
        factory: update,
    },
    PhaseSuite {
        name: "narrowed",
        factory: narrowed,
    },
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
];

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let mut failures = Vec::new();
    for (namespace, name) in [
        ("edgion-test", "delegation-dyn-parent"),
        ("edgion-deleg-dyn", "billing-child"),
    ] {
        match client.delete("HTTPRoute", Some(namespace), name).await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("{namespace}/{name}: HTTP {}", response.status())),
            Err(error) => failures.push(format!("{namespace}/{name}: {error}")),
        }
    }
    anyhow::ensure!(failures.is_empty(), "cleanup failed: {}", failures.join("; "));
    ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await
}

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let initial_dir = if ctx.resolve("initial").is_dir() {
            "initial/"
        } else {
            ""
        };
        let parent = format!("{initial_dir}HTTPRoute_edgion-test_delegation-dyn-parent.yaml");
        let child = format!("{initial_dir}HTTPRoute_edgion-deleg-dyn_billing-child.yaml");
        let result = async {
            cleanup(&ctx).await?;
            ctx.apply_dir(&parent).await?;
            ctx.apply_dir(&child).await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            ctx.apply_dir("updates/HTTPRoute_edgion-deleg-dyn_billing-child.yaml")
                .await?;
            ctx.wait_phase(SCENARIO, "update", Duration::from_secs(20)).await?;
            ctx.apply_dir("updates/parent/").await?;
            ctx.wait_phase(SCENARIO, "narrowed", Duration::from_secs(20)).await?;
            ctx.apply_dir(&parent).await?;
            ctx.wait_phase(SCENARIO, "update", Duration::from_secs(20)).await?;
            Ok(())
        }
        .await;
        let cleaned = cleanup(&ctx).await;
        match (result, cleaned) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!(
                "scenario failed: {error:#}; cleanup failed: {cleanup:#}"
            )),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "RouteDelegationDynamic",
    conf_path: "HTTPRoute/Delegation/DynamicTest",
    run,
    phases: PHASES,
};
