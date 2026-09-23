//! Global preflight policy updates on an unchanged HTTP listener.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Result};
use reqwest::Method;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const ORIGIN: &str = "https://app.example.com";
const CONVERGENCE: Duration = Duration::from_secs(20);
const IO_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
enum Generation {
    Initial,
    Updated,
}

async fn options(ctx: &TestContext, path: &str, standard: bool) -> Result<reqwest::Response> {
    let mut request = ctx
        .http_client
        .request(Method::OPTIONS, format!("{}/echo", ctx.http_url()))
        .header("Host", format!("{path}.preflight-policy.sandbox.example.com"))
        .timeout(Duration::from_secs(3));
    if standard {
        request = request
            .header("Origin", ORIGIN)
            .header("Access-Control-Request-Method", "GET");
    }
    Ok(request.send().await?)
}

async fn assert_generation(ctx: &TestContext, generation: Generation) -> Result<()> {
    let updated = matches!(generation, Generation::Updated);
    let plain = options(ctx, "fallback", false).await?;
    ensure!(
        plain.status().as_u16() == if updated { 205 } else { 405 },
        "plain OPTIONS expected {}, got {}",
        if updated { 205 } else { 405 },
        plain.status()
    );
    ensure!(
        !plain.headers().contains_key("access-control-allow-origin"),
        "global fallback added a CORS response header"
    );

    let standard = options(ctx, "fallback", true).await?;
    ensure!(
        standard.status().as_u16() == if updated { 205 } else { 204 },
        "standard preflight expected {}, got {}",
        if updated { 205 } else { 204 },
        standard.status()
    );
    ensure!(
        !standard.headers().contains_key("access-control-allow-origin"),
        "global fallback added a CORS response header"
    );

    for path in ["plugin", "native"] {
        let delegated = options(ctx, path, true).await?;
        ensure!(
            delegated.status().as_u16() == 204,
            "{path} CORS returned {}",
            delegated.status()
        );
        ensure!(
            delegated
                .headers()
                .get("access-control-allow-origin")
                .is_some_and(|value| value == ORIGIN),
            "{path} CORS did not own the preflight response"
        );
        let without_origin = options(ctx, path, false).await?;
        ensure!(
            without_origin.status().as_u16() == 405,
            "{path} CORS should pass OPTIONS without Origin upstream, got {}",
            without_origin.status()
        );
    }
    Ok(())
}

async fn persistent_preflight(stream: &mut TcpStream) -> Result<u16> {
    let request = format!(
        "OPTIONS /echo HTTP/1.1\r\nHost: fallback.preflight-policy.sandbox.example.com\r\nOrigin: {ORIGIN}\r\nAccess-Control-Request-Method: GET\r\nConnection: keep-alive\r\n\r\n"
    );
    timeout(IO_TIMEOUT, stream.write_all(request.as_bytes())).await??;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        ensure!(head.len() < 8192, "persistent preflight response header exceeded 8 KiB");
        let count = timeout(IO_TIMEOUT, stream.read(&mut byte)).await??;
        ensure!(count == 1, "persistent preflight connection closed before response");
        head.push(byte[0]);
    }
    let head = std::str::from_utf8(&head)?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow::anyhow!("persistent preflight response has no status"))?
        .parse()?;
    Ok(status)
}

struct GenerationSuite(Generation);

impl TestSuite for GenerationSuite {
    fn name(&self) -> &str {
        "PreflightPolicyHotReloadGeneration"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let generation = self.0;
        vec![TestCase::new(
            "preflight_policy_generation",
            "Preflight selection, fallback status, and route CORS ownership follow the current policy",
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

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
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
            let mut connection = timeout(
                IO_TIMEOUT,
                TcpStream::connect((ctx.test_ctx.target_host.as_str(), ctx.test_ctx.http_port)),
            )
            .await??;
            ensure!(
                persistent_preflight(&mut connection).await? == 204,
                "existing connection did not use the initial preflight policy"
            );
            ctx.apply_dir("updates").await?;
            ctx.wait_phase(SCENARIO, "updated", CONVERGENCE).await?;
            ensure!(
                persistent_preflight(&mut connection).await? == 205,
                "existing connection did not use the updated preflight policy"
            );
            Ok(())
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
    id: "PreflightPolicyHotReload",
    conf_path: "Gateway/PreflightPolicyHotReload/DynamicTest",
    run,
    phases: PHASES,
};
