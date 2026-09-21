//! Runtime EGC updates on unchanged listeners, including deletion defaults.
//! Loopback denial is not asserted: the shared integration topology deliberately
//! permits loopback independently of the EGC switch. L4TimeoutHotReload covers
//! the shared TCP/TLS relay timeout generation boundary separately.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::NoCertVerifier;
use anyhow::{ensure, Context, Result};
use rustls::pki_types::ServerName;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const HTTP_PORT: u16 = 31981;
const TLS_PORT: u16 = 31982;
const HOST: &str = "egc-runtime.sandbox.example.com";
const EGC: &str = "egc-runtime-hot-reload";
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const CONVERGENCE: Duration = Duration::from_secs(20);
type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

#[derive(Clone, Copy)]
enum Generation {
    Initial,
    Updated,
    Deleted,
}

async fn handshake(ctx: &TestContext, enable_sni: bool) -> Result<std::io::Result<TlsStream>> {
    let tcp = timeout(IO_TIMEOUT, TcpStream::connect((ctx.target_host.as_str(), TLS_PORT)))
        .await
        .context("TLS listener connect timed out")??;
    let mut config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
        .with_no_client_auth();
    config.enable_sni = enable_sni;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    timeout(IO_TIMEOUT, connector.connect(ServerName::try_from(HOST)?, tcp))
        .await
        .context("TLS handshake timed out")
}

async fn no_sni_connection(ctx: &TestContext) -> Result<TlsStream> {
    handshake(ctx, false).await?.context("no-SNI handshake rejected")
}

async fn echo(stream: &mut TlsStream) -> Result<()> {
    const PAYLOAD: &[u8] = b"egc-generation-echo";
    timeout(IO_TIMEOUT, stream.write_all(PAYLOAD)).await??;
    let mut response = [0u8; PAYLOAD.len()];
    timeout(IO_TIMEOUT, stream.read_exact(&mut response)).await??;
    ensure!(response == PAYLOAD, "TLS relay echo payload mismatch");
    Ok(())
}

async fn assert_generation(ctx: &TestContext, generation: Generation) -> Result<()> {
    let updated = matches!(generation, Generation::Updated);
    let client = reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .timeout(IO_TIMEOUT)
        .build()?;
    let url = format!("http://{}:{HTTP_PORT}/headers", ctx.target_host);
    let response = client
        .get(&url)
        .header("Host", HOST)
        .header("X-Forwarded-For", "192.0.2.123, 198.51.100.42")
        .send()
        .await?;
    // Deleting a referenced EGC independently makes plugin admission fail
    // closed before routing. Runtime defaults must not bypass that boundary.
    let expected = match generation {
        Generation::Initial => 200,
        Generation::Updated => 400,
        Generation::Deleted => 503,
    };
    ensure!(
        response.status().as_u16() == expected,
        "XFF expected {expected}, got {}",
        response.status()
    );
    let response = client
        .get(url)
        .header("Host", HOST)
        .header("Accept-Encoding", "gzip")
        .send()
        .await?;
    ensure!(
        response.status().as_u16()
            == if matches!(generation, Generation::Deleted) {
                503
            } else {
                200
            },
        "compression probe returned {}",
        response.status()
    );
    let compressed = response
        .headers()
        .get("content-encoding")
        .is_some_and(|value| value == "gzip");
    ensure!(compressed == updated, "gzip expected {updated}, got {compressed}");
    let body = response.bytes().await?;
    if !matches!(generation, Generation::Deleted) {
        ensure!(!body.is_empty(), "compression probe returned an empty body");
    }
    if updated {
        ensure!(
            body.starts_with(&[0x1f, 0x8b]),
            "gzip response missing gzip wire signature"
        );
    }

    if matches!(generation, Generation::Initial) {
        echo(&mut no_sni_connection(ctx).await?).await?;
    } else {
        // Prove certificate selection still works with explicit SNI, and routing
        // works while the EGC exists. A dead listener is not policy rejection.
        let mut explicit_sni = handshake(ctx, true).await??;
        if updated {
            echo(&mut explicit_sni).await?;
        }
        ensure!(
            handshake(ctx, false).await?.is_err(),
            "no-SNI handshake unexpectedly succeeded"
        );
    }
    Ok(())
}

struct GenerationSuite(Generation);
impl TestSuite for GenerationSuite {
    fn name(&self) -> &str {
        "EgcRuntimeHotReloadGeneration"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let generation = self.0;
        vec![TestCase::new(
            "egc_runtime_generation",
            "XFF, compression and no-SNI handshake use current EGC",
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
            // The static loader can preload any generation. Restore and prove
            // the initial runtime policy before holding the old connection.
            restore(&ctx).await?;
            let mut old = no_sni_connection(&ctx.test_ctx).await?;
            echo(&mut old).await?;
            ctx.apply_dir("updates").await?;
            ctx.wait_phase(SCENARIO, "updated", CONVERGENCE).await?;
            echo(&mut old)
                .await
                .context("old TLS connection changed during EGC update")?;
            restore(&ctx).await?;
            let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
            let response = client.delete("EdgionGatewayConfig", None, EGC).await?;
            ensure!(
                response.status().is_success(),
                "EGC deletion returned {}",
                response.status()
            );
            ctx.wait_phase(SCENARIO, "deleted", CONVERGENCE).await?;
            echo(&mut old)
                .await
                .context("old TLS connection changed during EGC deletion")?;
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
    id: "EgcRuntimeHotReload",
    conf_path: "Gateway/EgcRuntimeHotReload/DynamicTest",
    run,
    phases: PHASES,
};
