//! Plugin-owned Wasm state and tick authorization across configuration generations.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum Phase {
    Absent,
    Initial,
    Updated,
}

async fn check(ctx: &TestContext, phase: Phase) -> Result<()> {
    for (plugin, initially_allowed) in [("a", true), ("b", false)] {
        let response = ctx
            .http_client
            .get(format!("{}/headers", ctx.http_url()))
            .header("host", format!("wasm-runtime-{plugin}.example.com"))
            .header("x-wasm-read-ticks", "1")
            .header("x-wasm-read-callout", "1")
            .timeout(Duration::from_secs(3))
            .send()
            .await?;
        if matches!(phase, Phase::Absent) {
            ensure!(response.status() == 404, "plugin {plugin}: route still exists");
            continue;
        }
        ensure!(
            response.status() == 200,
            "plugin {plugin}: status {}",
            response.status()
        );
        let body: Value = response.json().await?;
        let counter = |name: &str| -> Result<u64> {
            body["headers"][name]
                .as_str()
                .with_context(|| format!("plugin {plugin}: missing {name}"))?
                .parse()
                .with_context(|| format!("plugin {plugin}: invalid {name}"))
        };
        let ticks = counter("x-wasm-ticks")?;
        let callouts = counter("x-wasm-calloutticks")?;
        // Require multiple tick rounds so an uninitialized runtime cannot pass
        // the denied-callout assertion merely because it has not started yet.
        ensure!(ticks >= 2, "plugin {plugin}: waiting for tick activity, got {ticks}");
        let allowed = matches!(phase, Phase::Initial) == initially_allowed;
        ensure!(
            if allowed { callouts > 0 } else { callouts == 0 },
            "plugin {plugin}: allowed={allowed}, ticks={ticks}, successful tick callouts={callouts}"
        );
    }
    Ok(())
}

struct Suite(Phase);

impl TestSuite for Suite {
    fn name(&self) -> &str {
        "WasmRuntimeOwnership"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "wasm_runtime_isolation",
            "Independent plugins retain independent shared data and tick authorization after reload",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx, phase).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )]
    }
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: || Box::new(Suite(Phase::Absent)),
    },
    PhaseSuite {
        name: "initial",
        factory: || Box::new(Suite(Phase::Initial)),
    },
    PhaseSuite {
        name: "updated",
        factory: || Box::new(Suite(Phase::Updated)),
    },
];

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let mut errors = Vec::new();
    for kind in ["HTTPRoute", "EdgionPlugins"] {
        for name in ["wasm-runtime-a", "wasm-runtime-b"] {
            match client.delete(kind, Some("edgion-default"), name).await {
                Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
                Ok(response) => errors.push(format!("{kind}/{name}: {}", response.status())),
                Err(error) => errors.push(format!("{kind}/{name}: {error}")),
            }
        }
    }
    ensure!(errors.is_empty(), "cleanup failed: {}", errors.join("; "));
    ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            // The static loader can preload dynamic fixtures. Always establish
            // absence before installing the first generation, including reruns.
            cleanup(&ctx).await?;
            ctx.apply_dir("initial").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(30)).await?;
            ctx.apply_dir("updates").await?;
            ctx.wait_phase(SCENARIO, "updated", Duration::from_secs(30)).await?;
            ctx.apply_dir("initial").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(30)).await
        }
        .await;
        let cleanup = cleanup(&ctx).await;
        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "WasmRuntimeOwnership",
    conf_path: "EdgionPlugins/Wasm/DynamicTest",
    run,
    phases: PHASES,
};
