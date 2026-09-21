//! Duplicate-Host policy updates on unchanged ordinary and redirect listeners.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::port_config::{PortConfig, PortKind};
use anyhow::{ensure, Context, Result};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const HOST: &str = "duplicate-host-policy.sandbox.example.com";
const EGC: &str = "duplicate-host-policy-hot-reload";
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const CONVERGENCE: Duration = Duration::from_secs(20);

#[derive(Clone, Copy)]
enum Generation {
    Initial,
    Updated,
    Deleted,
}

async fn duplicate_host_response(ctx: &TestContext, port: u16) -> Result<String> {
    let mut stream = timeout(IO_TIMEOUT, TcpStream::connect((ctx.target_host.as_str(), port)))
        .await
        .with_context(|| format!("duplicate-Host listener {port} connect timed out"))??;
    let request =
        format!("GET /health HTTP/1.1\r\nHost: {HOST}\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n");
    timeout(IO_TIMEOUT, stream.write_all(request.as_bytes()))
        .await
        .with_context(|| format!("duplicate-Host listener {port} write timed out"))??;
    let mut response = Vec::new();
    let read = timeout(IO_TIMEOUT, stream.read_to_end(&mut response))
        .await
        .with_context(|| format!("duplicate-Host listener {port} read timed out"))?;
    if let Err(error) = read {
        let complete_head = response.windows(4).any(|window| window == b"\r\n\r\n");
        if error.kind() != std::io::ErrorKind::ConnectionReset || !complete_head {
            return Err(error).with_context(|| format!("duplicate-Host listener {port} read failed"));
        }
    }
    String::from_utf8(response).context("duplicate-Host response was not UTF-8")
}

fn assert_status(response: &str, expected: u16, listener: &str) -> Result<()> {
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok());
    ensure!(
        status == Some(expected),
        "{listener} duplicate-Host request expected {expected}, got {}",
        response.lines().next().unwrap_or("empty response")
    );
    Ok(())
}

async fn assert_generation(ctx: &TestContext, generation: Generation) -> Result<()> {
    let escape_hatch = matches!(generation, Generation::Updated);
    let ordinary = duplicate_host_response(ctx, ctx.http_port).await?;
    assert_status(&ordinary, if escape_hatch { 200 } else { 400 }, "ordinary HTTP")?;
    let redirect_port = PortConfig::load()
        .map_err(anyhow::Error::msg)?
        .require_port("Gateway/DuplicateHostPolicyHotReload/Redirect", PortKind::Http)
        .map_err(anyhow::Error::msg)?;
    let redirect = duplicate_host_response(ctx, redirect_port).await?;
    assert_status(&redirect, if escape_hatch { 301 } else { 400 }, "HTTP redirect")?;
    if escape_hatch {
        ensure!(
            redirect
                .to_ascii_lowercase()
                .contains("\r\nlocation: https://duplicate-host-policy.sandbox.example.com/health\r\n"),
            "redirect response did not preserve the routable Host in Location"
        );
    }
    Ok(())
}

struct GenerationSuite(Generation);

impl TestSuite for GenerationSuite {
    fn name(&self) -> &str {
        "DuplicateHostPolicyHotReloadGeneration"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let generation = self.0;
        vec![TestCase::new(
            "duplicate_host_policy_generation",
            "Ordinary and redirect listeners use the current duplicate-Host policy",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match assert_generation(&ctx, generation).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}

fn initial() -> Box<dyn TestSuite> {
    Box::new(GenerationSuite(Generation::Initial))
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(GenerationSuite(Generation::Updated))
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(GenerationSuite(Generation::Deleted))
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
        name: "deleted",
        factory: deleted,
    },
];

async fn restore(ctx: &ScenarioCtx) -> Result<()> {
    ctx.apply_dir("initial").await?;
    ctx.wait_phase(SCENARIO, "initial", CONVERGENCE).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            restore(&ctx).await?;
            ctx.apply_dir("updates").await?;
            ctx.wait_phase(SCENARIO, "updated", CONVERGENCE).await?;
            restore(&ctx).await?;
            let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
            let response = client.delete("EdgionGatewayConfig", None, EGC).await?;
            ensure!(
                response.status().is_success(),
                "EGC deletion returned {}",
                response.status()
            );
            ctx.wait_phase(SCENARIO, "deleted", CONVERGENCE).await
        }
        .await;
        let restored = restore(&ctx).await;
        match (result, restored) {
            (Ok(()), restored) => restored,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(restore)) => Err(anyhow::anyhow!("{error:#}; restore also failed: {restore:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "DuplicateHostPolicyHotReload",
    conf_path: "Gateway/DuplicateHostPolicyHotReload/DynamicTest",
    run,
    phases: PHASES,
};
