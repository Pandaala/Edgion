//! Live hot-reload coverage for `EdgionGatewayConfig.spec.tcpTimeout.idleTimeout`.
//!
//! A dedicated GatewayClass pins these listeners to `l4-timeout-egc`. Updating
//! only that EGC must change newly accepted TCP, TLS-terminate, and TLS
//! passthrough connections without restarting the Gateway or replacing listeners.
//! Connections that already completed routing keep the generation copied at
//! `process_new`.
//!
//! `connectTimeout` is published through the same snapshot. This scenario does
//! not add connect-timeout wire coverage: holding upstream establishment open
//! without OS/network black-hole timing is not deterministic across Linux CI and
//! macOS. Unit tests cover connect-timeout publication and capture.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::make_tls_connector;
use anyhow::{ensure, Context, Result};
use rustls::pki_types::ServerName;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

const EGC_NAME: &str = "l4-timeout-egc";
const K8S_GATEWAY_HOST: &str = "l4-timeout-gateway.edgion-system.svc.cluster.local";
const SNI: &str = "l4-timeout.sandbox.example.com";
const TCP_PORT: u16 = 31978;
const TERMINATE_PORT: u16 = 31979;
const PASSTHROUGH_PORT: u16 = 31980;
const LONG_IDLE: &str = "30s";
const SHORT_IDLE: &str = "1s";
const CONVERGE_DEADLINE: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const SHORT_IDLE_MIN: Duration = Duration::from_millis(700);
const SHORT_IDLE_MAX: Duration = Duration::from_millis(3500);
const PUBLICATION_PROBE: Duration = Duration::from_millis(1800);
const OLD_CONN_IDLE: Duration = Duration::from_millis(2500);
const TCP_ECHO: &[u8] = b"l4-timeout-echo";
const HEALTH_REQUEST: &[u8] =
    b"GET /health HTTP/1.1\r\nHost: l4-timeout.sandbox.example.com\r\nConnection: keep-alive\r\n\r\n";

#[derive(Clone, Copy)]
enum PathKind {
    Tcp,
    Terminate,
    Passthrough,
}

impl PathKind {
    fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Terminate => "tls-terminate",
            Self::Passthrough => "tls-passthrough",
        }
    }

    fn port(self) -> u16 {
        match self {
            Self::Tcp => TCP_PORT,
            Self::Terminate => TERMINATE_PORT,
            Self::Passthrough => PASSTHROUGH_PORT,
        }
    }

    fn all() -> Vec<Self> {
        let mut paths = vec![Self::Tcp, Self::Passthrough];
        if crate::suites::tls_route::tls_route_terminate_tests_enabled() {
            paths.push(Self::Terminate);
        }
        paths
    }
}

trait RelayStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RelayStream for T {}

struct HeldConn {
    kind: PathKind,
    stream: Box<dyn RelayStream>,
}

struct ReadySuite;

impl TestSuite for ReadySuite {
    fn name(&self) -> &str {
        "L4TimeoutHotReloadReady"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "l4_timeout_listeners_ready",
            "Dedicated TCP, TLS terminate, and TLS passthrough listeners echo",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    for kind in PathKind::all() {
                        if let Err(error) = probe_once(&ctx, kind).await {
                            return TestResult::failed(start.elapsed(), format!("{}: {error:#}", kind.name()));
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
        .with_timeout(Duration::from_secs(8))]
    }
}

fn ready() -> Box<dyn TestSuite> {
    Box::new(ReadySuite)
}

static PHASES: &[PhaseSuite] = &[PhaseSuite {
    name: "ready",
    factory: ready,
}];

async fn connect_raw(ctx: &TestContext, port: u16) -> Result<TcpStream> {
    let host = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
        K8S_GATEWAY_HOST
    } else {
        &ctx.target_host
    };
    let addr = format!("{host}:{port}");
    let stream = timeout(SETUP_TIMEOUT, TcpStream::connect(&addr))
        .await
        .with_context(|| format!("connect to {addr} timed out"))?
        .with_context(|| format!("connect to {addr} failed"))?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

async fn open_conn(ctx: &TestContext, kind: PathKind) -> Result<HeldConn> {
    let tcp = connect_raw(ctx, kind.port()).await?;
    let stream: Box<dyn RelayStream> = match kind {
        PathKind::Tcp => Box::new(tcp),
        PathKind::Terminate | PathKind::Passthrough => {
            let connector = make_tls_connector();
            let sni = ServerName::try_from(SNI).expect("static SNI is a valid DNS name");
            let tls = timeout(SETUP_TIMEOUT, connector.connect(sni, tcp))
                .await
                .with_context(|| format!("{} handshake timed out", kind.name()))?
                .with_context(|| format!("{} handshake failed", kind.name()))?;
            Box::new(tls)
        }
    };
    Ok(HeldConn { kind, stream })
}

fn header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn content_length(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

fn chunked_complete(buf: &[u8], header_end: usize) -> bool {
    buf[header_end..].windows(5).any(|window| window == b"0\r\n\r\n")
}

async fn read_http1_response(stream: &mut (impl AsyncRead + Unpin)) -> Result<String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "passthrough HTTP response timed out");
        let n = timeout(remaining, stream.read(&mut tmp))
            .await
            .context("passthrough read timed out")?
            .context("passthrough read failed")?;
        ensure!(n > 0, "passthrough closed with no HTTP response");
        buf.extend_from_slice(&tmp[..n]);
        if let Some(end) = header_end(&buf) {
            let headers = std::str::from_utf8(&buf[..end]).unwrap_or("");
            let complete = if let Some(len) = content_length(headers) {
                buf.len() >= end + len
            } else if headers.to_ascii_lowercase().contains("transfer-encoding: chunked") {
                chunked_complete(&buf, end)
            } else {
                true
            };
            if complete {
                return Ok(String::from_utf8_lossy(&buf).into_owned());
            }
        }
        ensure!(buf.len() < 16 * 1024, "passthrough HTTP response too large");
    }
}

async fn echo(conn: &mut HeldConn) -> Result<()> {
    match conn.kind {
        PathKind::Tcp | PathKind::Terminate => {
            timeout(IO_TIMEOUT, conn.stream.write_all(TCP_ECHO))
                .await
                .context("echo write timed out")?
                .context("echo write failed")?;
            let mut buf = [0u8; 64];
            let n = timeout(IO_TIMEOUT, conn.stream.read(&mut buf))
                .await
                .context("echo read timed out")?
                .context("echo read failed")?;
            ensure!(n > 0, "echo closed with no data");
            ensure!(&buf[..n] == TCP_ECHO, "echo payload mismatch");
        }
        PathKind::Passthrough => {
            timeout(IO_TIMEOUT, conn.stream.write_all(HEALTH_REQUEST))
                .await
                .context("passthrough write timed out")?
                .context("passthrough write failed")?;
            let body = read_http1_response(&mut conn.stream).await?;
            ensure!(
                body.contains("200") || body.contains("OK") || body.contains("health"),
                "passthrough HTTP response was not healthy: {}",
                &body[..body.len().min(120)]
            );
        }
    }
    Ok(())
}

async fn probe_once(ctx: &TestContext, kind: PathKind) -> Result<()> {
    let mut conn = open_conn(ctx, kind).await?;
    echo(&mut conn).await
}

async fn wait_idle_close(conn: &mut HeldConn) -> Result<Duration> {
    let start = Instant::now();
    let mut buf = [0u8; 16];
    match timeout(SHORT_IDLE_MAX, conn.stream.read(&mut buf)).await {
        Ok(Ok(0)) | Ok(Err(_)) => {
            let waited = start.elapsed();
            ensure!(
                waited >= SHORT_IDLE_MIN,
                "{} closed too early after {waited:?}",
                conn.kind.name()
            );
            Ok(waited)
        }
        Ok(Ok(n)) => anyhow::bail!(
            "{} received {n} unexpected bytes while waiting for the short idle timeout",
            conn.kind.name()
        ),
        Err(_) => anyhow::bail!(
            "{} stayed open for {SHORT_IDLE_MAX:?}; short idle timeout did not fire",
            conn.kind.name()
        ),
    }
}

async fn observed_idle_timeout(ctx: &ScenarioCtx) -> Result<String> {
    let mut request = ctx
        .test_ctx
        .http_client
        .get(format!(
            "{}/configclient/EdgionGatewayConfig/list",
            ctx.gateway_url.trim_end_matches('/')
        ))
        .timeout(Duration::from_secs(3));
    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        if !token.trim().is_empty() {
            request = request.bearer_auth(token);
        } else if let Some(token) = ctx.test_ctx.admin_token.as_deref() {
            request = request.bearer_auth(token);
        }
    } else if let Some(token) = ctx.test_ctx.admin_token.as_deref() {
        request = request.bearer_auth(token);
    }
    let body: Value = request.send().await?.error_for_status()?.json().await?;
    ensure!(body["success"] == true, "gateway EdgionGatewayConfig list failed");
    let items = body["data"]
        .as_array()
        .context("gateway EdgionGatewayConfig list has no data array")?;
    let config = items
        .iter()
        .find(|item| item["metadata"]["name"] == EGC_NAME)
        .with_context(|| format!("{EGC_NAME} missing from gateway configclient"))?;
    config["spec"]["tcpTimeout"]["idleTimeout"]
        .as_str()
        .map(str::to_string)
        .context("published EGC has no spec.tcpTimeout.idleTimeout")
}

async fn wait_configclient_idle_timeout(ctx: &ScenarioCtx, expected: &str) -> Result<()> {
    let deadline = Instant::now() + CONVERGE_DEADLINE;
    loop {
        let last = match observed_idle_timeout(ctx).await {
            Ok(value) if value == expected => return Ok(()),
            Ok(value) => format!("observed idleTimeout {value}"),
            Err(error) => format!("{error:#}"),
        };
        ensure!(
            Instant::now() < deadline,
            "gateway configclient idleTimeout did not converge to {expected}: {last}"
        );
        sleep(POLL_INTERVAL).await;
    }
}

/// ConfigClient can expose the updated resource before ConfHandler's 100ms
/// debounce publishes `ResolvedL4Timeouts`. Probe a disposable TCP connection
/// until the live relay matches the expected generation.
async fn probe_published_idle(ctx: &TestContext, want_short: bool) -> Result<bool> {
    let mut conn = open_conn(ctx, PathKind::Tcp).await?;
    echo(&mut conn).await?;
    let start = Instant::now();
    let mut buf = [0u8; 16];
    match timeout(PUBLICATION_PROBE, conn.stream.read(&mut buf)).await {
        Ok(Ok(0)) | Ok(Err(_)) => {
            let waited = start.elapsed();
            Ok(want_short && waited >= SHORT_IDLE_MIN)
        }
        Ok(Ok(_)) => Ok(false),
        Err(_) => Ok(!want_short),
    }
}

async fn wait_published_idle(ctx: &ScenarioCtx, expected: &str) -> Result<()> {
    wait_configclient_idle_timeout(ctx, expected).await?;
    let want_short = expected == SHORT_IDLE;
    let deadline = Instant::now() + CONVERGE_DEADLINE;
    loop {
        let last = match probe_published_idle(&ctx.test_ctx, want_short).await {
            Ok(true) => return Ok(()),
            Ok(false) => "tcp probe still on the previous idle generation".to_string(),
            Err(error) => format!("{error:#}"),
        };
        ensure!(
            Instant::now() < deadline,
            "resolved L4 idle snapshot did not converge to {expected}: {last}"
        );
        sleep(POLL_INTERVAL).await;
    }
}

async fn restore_long(ctx: &ScenarioCtx) -> Result<()> {
    ctx.apply_dir("updates/to_long").await?;
    wait_published_idle(ctx, LONG_IDLE).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        if !crate::suites::tls_route::tls_route_terminate_tests_enabled() {
            crate::suites::tls_route::report_terminate_deferred(
                "L4TimeoutHotReload/TLS terminate new and retained connections",
            );
        }
        let result = async {
            ctx.apply_dir("initial").await?;
            ctx.wait_phase(SCENARIO, "ready", CONVERGE_DEADLINE).await?;
            wait_published_idle(&ctx, LONG_IDLE).await?;

            let mut old = Vec::new();
            for kind in PathKind::all() {
                let mut conn = open_conn(&ctx.test_ctx, kind).await?;
                echo(&mut conn)
                    .await
                    .with_context(|| format!("{} pre-update exchange failed", kind.name()))?;
                old.push(conn);
            }

            ctx.apply_dir("updates/to_short").await?;
            wait_published_idle(&ctx, SHORT_IDLE).await?;

            let idle_started = Instant::now();
            for kind in PathKind::all() {
                let mut fresh = open_conn(&ctx.test_ctx, kind).await?;
                echo(&mut fresh)
                    .await
                    .with_context(|| format!("{} post-update setup exchange failed", kind.name()))?;
                let waited = wait_idle_close(&mut fresh).await.with_context(|| {
                    format!("{} new connection did not observe the short idle timeout", kind.name())
                })?;
                println!(
                    "L4TimeoutHotReload: {} new connection closed after {waited:?}",
                    kind.name()
                );
            }

            if let Some(remaining) = OLD_CONN_IDLE.checked_sub(idle_started.elapsed()) {
                sleep(remaining).await;
            }
            for conn in &mut old {
                echo(conn).await.with_context(|| {
                    format!(
                        "{} pre-update connection was not usable after the short idle window",
                        conn.kind.name()
                    )
                })?;
            }
            Ok(())
        }
        .await;

        let restored = restore_long(&ctx).await;
        match (result, restored) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.context("failed to restore the long L4 idle timeout")),
            (Err(error), Err(restore_error)) => Err(error.context(format!("restore also failed: {restore_error:#}"))),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "L4TimeoutHotReload",
    conf_path: "TCPRoute/L4TimeoutHotReload/DynamicTest",
    run,
    phases: PHASES,
};
