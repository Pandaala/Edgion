//! Wire coverage for route managers retained by the original listener proxies.
//!
//! The listener ports are bound at process startup. Removing their Gateway and
//! Routes clears the old managers; recreating the same keys publishes into new
//! managers and does not make the old serving proxies route again.

use crate::dynamic::scenario::{Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::suites::grpc_route::test::test_service_client::TestServiceClient;
use crate::suites::grpc_route::test::HelloRequest;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout, Instant};

const NAMESPACE: &str = "edgion-test";
const GATEWAY: &str = "retained-handles-gateway";
const HTTP_ROUTE: &str = "retained-handles-http";
const GRPC_ROUTE: &str = "retained-handles-grpc";
const TCP_ROUTE: &str = "retained-handles-tcp";
const HTTP_HOST: &str = "retained-http.sandbox.example.com";
const GRPC_HOST: &str = "retained-grpc.sandbox.example.com";
const HTTP_PORT: u16 = 31986;
const GRPC_PORT: u16 = 31987;
const TCP_PORT: u16 = 31988;
const DEADLINE: Duration = Duration::from_secs(20);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const ECHO: &[u8] = b"retained-handle-probe";

#[derive(Clone, Copy, Debug)]
enum Generation {
    Present,
    Absent,
    Recreated,
}

#[derive(Clone, Copy, Debug)]
struct RouteStats {
    http_routes: u64,
    grpc_routes: u64,
    tcp_routes: u64,
    http_listeners: u64,
    grpc_listeners: u64,
    tcp_listeners: u64,
    http_domains: u64,
    grpc_units: u64,
    tcp_table_routes: u64,
}

fn gateway_host(ctx: &ScenarioCtx) -> &str {
    if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
        "retained-handles-gateway.edgion-system.svc.cluster.local"
    } else {
        &ctx.test_ctx.target_host
    }
}

async fn listed(ctx: &ScenarioCtx, kind: &str, name: &str) -> Result<bool> {
    let url = format!("{}/configclient/{kind}/list", ctx.gateway_url.trim_end_matches('/'));
    let request = gateway_auth(ctx, ctx.test_ctx.http_client.get(url).timeout(IO_TIMEOUT));
    let body: Value = request.send().await?.error_for_status()?.json().await?;
    ensure!(body["success"] == true, "gateway {kind} list failed");
    let items = body["data"].as_array().context("gateway list has no data array")?;
    Ok(items
        .iter()
        .any(|item| item["metadata"]["namespace"] == NAMESPACE && item["metadata"]["name"] == name))
}

fn gateway_auth(ctx: &ScenarioCtx, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let token = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| ctx.test_ctx.admin_token.clone());
    match token {
        Some(token) if !token.is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

fn count(data: &Value, family: &str, field: &str) -> Result<u64> {
    data[family][field]
        .as_u64()
        .with_context(|| format!("store stats has no {family}.{field}"))
}

async fn route_stats(ctx: &ScenarioCtx) -> Result<RouteStats> {
    let url = format!("{}/api/v1/debug/store-stats", ctx.gateway_url.trim_end_matches('/'));
    let request = gateway_auth(ctx, ctx.test_ctx.http_client.get(url).timeout(IO_TIMEOUT));
    let body: Value = request.send().await?.error_for_status()?.json().await?;
    ensure!(body["success"] == true, "gateway store stats failed");
    let data = &body["data"];
    Ok(RouteStats {
        http_routes: count(data, "http_routes", "http_routes")?,
        grpc_routes: count(data, "grpc_routes", "grpc_routes")?,
        tcp_routes: count(data, "tcp_routes", "route_cache")?,
        http_listeners: count(data, "http_routes", "listener_count")?,
        grpc_listeners: count(data, "grpc_routes", "listener_count")?,
        tcp_listeners: count(data, "tcp_routes", "listener_count")?,
        http_domains: count(data, "http_routes", "exact_domains")?,
        grpc_units: count(data, "grpc_routes", "route_units_cache")?,
        tcp_table_routes: count(data, "tcp_routes", "total_routes_in_tables")?,
    })
}

fn assert_stats(current: RouteStats, absent: RouteStats, present: bool) -> Result<()> {
    let expected = if present { 1 } else { 0 };
    // Each route manager registers all three Gateway listener keys.
    ensure!(
        current.http_routes == absent.http_routes + expected,
        "HTTP route cache: {current:?}, absent: {absent:?}"
    );
    ensure!(
        current.grpc_routes == absent.grpc_routes + expected,
        "gRPC route cache: {current:?}, absent: {absent:?}"
    );
    ensure!(
        current.tcp_routes == absent.tcp_routes + expected,
        "TCP route cache: {current:?}, absent: {absent:?}"
    );
    ensure!(
        current.http_listeners == absent.http_listeners + 3 * expected,
        "HTTP manager registry: {current:?}, absent: {absent:?}"
    );
    ensure!(
        current.grpc_listeners == absent.grpc_listeners + 3 * expected,
        "gRPC manager registry: {current:?}, absent: {absent:?}"
    );
    ensure!(
        current.tcp_listeners == absent.tcp_listeners + 3 * expected,
        "TCP manager registry: {current:?}, absent: {absent:?}"
    );
    ensure!(
        current.http_domains == absent.http_domains + expected,
        "HTTP domain table: {current:?}, absent: {absent:?}"
    );
    ensure!(
        current.grpc_units == absent.grpc_units + expected,
        "gRPC compiled units: {current:?}, absent: {absent:?}"
    );
    ensure!(
        current.tcp_table_routes == absent.tcp_table_routes + expected,
        "TCP route table: {current:?}, absent: {absent:?}"
    );
    Ok(())
}

async fn assert_publication(ctx: &ScenarioCtx, present: bool) -> Result<()> {
    for (kind, name) in [
        ("Gateway", GATEWAY),
        ("HTTPRoute", HTTP_ROUTE),
        ("GRPCRoute", GRPC_ROUTE),
        ("TCPRoute", TCP_ROUTE),
    ] {
        let found = listed(ctx, kind, name).await?;
        ensure!(
            found == present,
            "{kind}/{name} publication: expected present={present}, found={found}"
        );
    }
    Ok(())
}

async fn http_status(ctx: &ScenarioCtx) -> Result<u16> {
    let response = ctx
        .test_ctx
        .http_client
        .get(format!("http://{}:{HTTP_PORT}/health", gateway_host(ctx)))
        .header("Host", HTTP_HOST)
        .timeout(IO_TIMEOUT)
        .send()
        .await?;
    Ok(response.status().as_u16())
}

async fn grpc_reached_backend(ctx: &ScenarioCtx) -> Result<bool> {
    let url = format!("http://{}:{GRPC_PORT}", gateway_host(ctx));
    let origin: http::Uri = format!("http://{GRPC_HOST}:{GRPC_PORT}").parse()?;
    let endpoint = tonic::transport::Endpoint::from_shared(url)?
        .origin(origin)
        .timeout(IO_TIMEOUT);
    let channel = timeout(IO_TIMEOUT, endpoint.connect()).await??;
    let mut client = TestServiceClient::new(channel);
    let request = tonic::Request::new(HelloRequest {
        name: "RetainedHandle".to_string(),
    });
    match timeout(IO_TIMEOUT, client.say_hello(request)).await? {
        Ok(response) => {
            ensure!(
                response.into_inner().server_addr == "0.0.0.0:30021",
                "gRPC response came from an unexpected backend"
            );
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

async fn tcp_reached_backend(ctx: &ScenarioCtx) -> Result<bool> {
    let addr = format!("{}:{TCP_PORT}", gateway_host(ctx));
    let mut stream = timeout(IO_TIMEOUT, TcpStream::connect(&addr))
        .await
        .with_context(|| format!("TCP listener {addr} did not accept a connection"))??;
    if !matches!(timeout(IO_TIMEOUT, stream.write_all(ECHO)).await, Ok(Ok(()))) {
        return Ok(false);
    }
    let mut received = [0u8; ECHO.len()];
    match timeout(IO_TIMEOUT, stream.read_exact(&mut received)).await {
        Ok(Ok(_)) => Ok(received == ECHO),
        Ok(Err(_)) | Err(_) => Ok(false),
    }
}

async fn assert_traffic(ctx: &ScenarioCtx, generation: Generation) -> Result<()> {
    let expected_present = matches!(generation, Generation::Present);
    let status = http_status(ctx).await?;
    ensure!(
        status == if expected_present { 200 } else { 404 },
        "HTTP listener returned {status} for {generation:?}"
    );
    let grpc_backend = grpc_reached_backend(ctx).await?;
    ensure!(grpc_backend == expected_present, "gRPC backend reachability mismatch");
    let tcp_backend = tcp_reached_backend(ctx).await?;
    ensure!(tcp_backend == expected_present, "TCP backend reachability mismatch");
    Ok(())
}

/// Store-stat expectation checked on every poll, so registry retirement that
/// lags the routing outcome (for example an HTTPRoute delete applied before its
/// Gateway delete, retired only by the later full listener rebuild) converges
/// within the deadline instead of being sampled once.
#[derive(Clone, Copy)]
enum StatsExpectation {
    None,
    /// Current stats must be exactly one generation above this baseline.
    OneGenerationAbove(RouteStats),
    /// Current stats must be exactly one generation below this baseline.
    OneGenerationBelow(RouteStats),
}

async fn wait_generation(ctx: &ScenarioCtx, generation: Generation, expect: StatsExpectation) -> Result<RouteStats> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let result: Result<RouteStats> = async {
            assert_publication(ctx, !matches!(generation, Generation::Absent)).await?;
            let current = route_stats(ctx).await?;
            match expect {
                StatsExpectation::None => {}
                StatsExpectation::OneGenerationAbove(baseline) => assert_stats(current, baseline, true)?,
                StatsExpectation::OneGenerationBelow(baseline) => assert_stats(baseline, current, true)?,
            }
            assert_traffic(ctx, generation).await?;
            Ok(current)
        }
        .await;
        match result {
            Ok(current) => return Ok(current),
            Err(error) if Instant::now() >= deadline => {
                return Err(error).context("retained listener generation did not converge");
            }
            Err(_) => sleep(POLL_INTERVAL).await,
        }
    }
}

async fn remove_routes_and_gateway(ctx: &ScenarioCtx) -> Result<()> {
    // Remove logical ownership while the routes are still configured, then
    // remove the routes. The final route delta must retire the old handles.
    ctx.delete_resources("delete/gateway.txt").await?;
    ctx.delete_resources("delete/routes.txt").await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        // Full-suite startup preloads initial fixtures. Applying them again also
        // supports a targeted run on the same shared process topology.
        ctx.apply_dir("initial").await?;
        let initial = wait_generation(&ctx, Generation::Present, StatsExpectation::None).await?;

        let result = async {
            remove_routes_and_gateway(&ctx).await?;
            let absent =
                wait_generation(&ctx, Generation::Absent, StatsExpectation::OneGenerationBelow(initial)).await?;
            ctx.apply_dir("initial").await?;
            wait_generation(
                &ctx,
                Generation::Recreated,
                StatsExpectation::OneGenerationAbove(absent),
            )
            .await?;
            Ok(())
        }
        .await;

        let cleanup = async {
            if result.is_err() {
                ctx.apply_dir("initial").await?;
            }
            remove_routes_and_gateway(&ctx).await
        }
        .await;
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.context("retained listener cleanup failed")),
            (Err(error), Err(cleanup_error)) => Err(error.context(format!("cleanup also failed: {cleanup_error:#}"))),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "RetainedListenerHandles",
    conf_path: "Gateway/RetainedListenerHandles/DynamicTest",
    run,
    phases: &[],
};
