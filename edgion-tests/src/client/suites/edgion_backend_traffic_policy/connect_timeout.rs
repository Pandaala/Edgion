// EdgionBackendTrafficPolicy/ConnectTimeout scenario.
//
// End-to-end coverage for `EdgionBackendTrafficPolicy.spec.connection.connectTimeout`
// (see docs/superpowers/specs/2026-07-22-per-backend-connect-timeout-design.md),
// against the fixtures in
// edgion-tests/integration/conf/EdgionBackendTrafficPolicy/ConnectTimeout/:
//
//   Gateway ct-gateway (port from ports.json, listener "http")
//     hostname "connect-timeout.example.com"
//     /blackhole -> ct-svc-a (10.255.255.1:1, a non-routable blackhole address)
//     /ok        -> ct-svc-b (shared plain-HTTP echo backend, 127.0.0.1:30001)
//   EdgionBackendTrafficPolicy ct-policy on Service ct-svc-a:
//     connection { connectTimeout: 300ms }
//
// The base EdgionGatewayConfig sets defaultConnectTimeout: 5s. ct-svc-a has no
// route in the sky to a live host, so without the override the gateway would
// hang the connect attempt for the full 5s before failing; the per-backend
// override must make it fail at ~300ms instead. ct-svc-b carries no BTP
// override and must keep working normally — proving the divergence is truly
// per-backend, not a global regression.
//
// Coverage map to spec "Testing / Integration":
//   1. A request to the blackholed backend fails fast (~connectTimeout), well
//      under the global defaultConnectTimeout -> test_blackhole_backend_fails_fast
//   2. A request to the non-overridden backend still succeeds normally,
//      proving the divergence -> test_normal_backend_still_works

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

pub struct ConnectTimeoutTestSuite;

const HOST: &str = "connect-timeout.example.com";

impl ConnectTimeoutTestSuite {
    /// Local backend only; the Controller and Gateway remain the shared topology.
    /// JoinSet owns the listener task and aborts it on every error/cancellation path.
    fn test_end_to_end_deadlines_bound_tls_establishment() -> TestCase {
        TestCase::new(
            "end_to_end_deadlines_bound_tls_establishment",
            "Request and gRPC deadlines terminate a confirmed stalled TLS handshake without retry",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if !ctx.gateway || std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            "skipped: requires the shared local gateway topology and suite-owned loopback backend"
                                .to_string(),
                        );
                    }
                    let result = run_stalled_tls_deadlines(&ctx).await;
                    match result {
                    Ok(()) => TestResult::passed_with_message(start.elapsed(),
                        "All three deadline cases observed ClientHello, bounded termination, canonical diagnostics and one attempt".to_string()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
                })
            },
        )
    }

    /// (1) The blackholed backend (ct-svc-a) carries a 300ms connectTimeout
    /// override. A request to it must come back with a connect-failure status
    /// (502/503/504 depending on how the data plane classifies the connect
    /// error) comfortably under the global 5s defaultConnectTimeout, and not
    /// suspiciously instant either (the deadline must actually be enforced,
    /// not short-circuited by some other error path).
    fn test_blackhole_backend_fails_fast() -> TestCase {
        TestCase::new(
            "blackhole_backend_fails_fast",
            "A request to the BTP-overridden blackhole backend fails fast at ~connectTimeout, not the global defaultConnectTimeout",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/blackhole", ctx.http_url());

                    let resp = match ctx.http_client.get(&url).header("Host", HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    };
                    let elapsed = start.elapsed();
                    let status = resp.status().as_u16();

                    if !(500..600).contains(&status) {
                        return TestResult::failed(
                            elapsed,
                            format!("expected a connect-failure status (5xx) from the blackhole backend, got {status}"),
                        );
                    }
                    // Comfortably under the global 5s defaultConnectTimeout: proves the
                    // per-backend 300ms override actually took effect instead of the
                    // gateway hanging for the full global deadline.
                    if elapsed >= std::time::Duration::from_secs(2) {
                        return TestResult::failed(
                            elapsed,
                            format!(
                                "expected the connect failure within ~300ms (well under the 5s global default), \
                                 took {elapsed:?}"
                            ),
                        );
                    }
                    // NOTE: intentionally no lower-bound timing assertion. On a runner
                    // whose host sits on a 10.0.0.0/8 network with no route to the
                    // blackhole, connect() can return ENETUNREACH/EHOSTUNREACH in well
                    // under the 300ms deadline — a legitimate fast connect failure, not a
                    // bug. The invariant this scenario proves is the per-backend
                    // DIVERGENCE: the overridden backend fails fast (< 2s, far under the
                    // 5s global default) while the non-overridden sibling still works.
                    TestResult::passed_with_message(
                        elapsed,
                        format!("blackhole backend failed fast with status {status} in {elapsed:?}"),
                    )
                })
            },
        )
    }

    /// (2) ct-svc-b carries no connect-timeout override, so it must keep
    /// working normally — proving the override applies only to the targeted
    /// Service, not globally.
    fn test_normal_backend_still_works() -> TestCase {
        TestCase::new(
            "normal_backend_still_works",
            "A request to the non-overridden backend succeeds normally",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/ok", ctx.http_url());

                    let resp = match ctx.http_client.get(&url).header("Host", HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    };
                    let elapsed = start.elapsed();
                    let status = resp.status().as_u16();

                    if status != 200 {
                        return TestResult::failed(
                            elapsed,
                            format!("expected 200 from the non-overridden backend, got {status}"),
                        );
                    }

                    TestResult::passed_with_message(elapsed, format!("normal backend responded 200 in {elapsed:?}"))
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for ConnectTimeoutTestSuite {
    fn name(&self) -> &str {
        "EdgionBackendTrafficPolicy/ConnectTimeout"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackendTrafficPolicy/ConnectTimeout")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_blackhole_backend_fails_fast(),
            Self::test_normal_backend_still_works(),
            Self::test_end_to_end_deadlines_bound_tls_establishment(),
        ]
    }
}

async fn run_stalled_tls_deadlines(ctx: &TestContext) -> Result<(), String> {
    use crate::port_config::{PortConfig, PortKind};
    const DEADLINE_HOST: &str = "connect-deadlines.example.com";
    let port = PortConfig::load()?.require_port("EdgionBackendTrafficPolicy/ConnectTimeout", PortKind::Tcp)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .http2_prior_knowledge()
        .resolve(DEADLINE_HOST, ([127, 0, 0, 1], ctx.http_port).into())
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| format!("build deadline client: {e}"))?;

    for (path, grpc, diagnostic) in [
        ("request-deadline", false, "RequestTimeoutExceeded"),
        ("grpc-deadline", true, "GrpcClientDeadlineExceeded"),
        ("both-deadlines", true, "RequestTimeoutExceeded"),
    ] {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|e| format!("bind TLS-stall listener: {e}"))?;
        let mut backend = tokio::task::JoinSet::new();
        backend.spawn(async move {
            tokio::time::timeout(Duration::from_secs(4), async move {
                let (mut stream, _) = listener.accept().await.map_err(|e| format!("accept TLS: {e}"))?;
                let mut prefix = [0u8; 6];
                stream
                    .read_exact(&mut prefix)
                    .await
                    .map_err(|e| format!("read ClientHello: {e}"))?;
                if prefix[0] != 22 || prefix[1] != 3 || prefix[5] != 1 {
                    return Err("backend did not receive a TLS ClientHello".to_string());
                }
                // Never answer the handshake; observe cancellation through EOF/reset.
                let mut buf = [0u8; 4096];
                let mut total = prefix.len();
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) => return Ok(()),
                        Ok(n) => {
                            total += n;
                            if total > 64 * 1024 {
                                return Err("TLS-stall input exceeded 64 KiB".to_string());
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return Ok(()),
                        Err(e) => return Err(format!("observe handshake cancellation: {e}")),
                    }
                }
            })
            .await
            .map_err(|_| "TLS-stall backend did not close within 4s".to_string())?
        });

        let trace_id = format!("connect-deadline-{}", uuid::Uuid::new_v4());
        let mut request = client
            .post(format!("http://{DEADLINE_HOST}:{}/{path}", ctx.http_port))
            .header(
                "traceparent",
                crate::access_log_client::traceparent_for_marker(&trace_id),
            )
            .header("access_log", "test_store");
        if grpc {
            request = request
                .header("content-type", "application/grpc")
                .header("grpc-timeout", "300m")
                .body(vec![0u8; 5]);
        }
        let began = Instant::now();
        let response = request
            .send()
            .await
            .map_err(|e| format!("{path}: request failed: {e}"))?;
        let elapsed = began.elapsed();
        let expected_status = if grpc { 200 } else { 504 };
        if response.status().as_u16() != expected_status {
            return Err(format!(
                "{path}: expected HTTP {expected_status}, got {}",
                response.status()
            ));
        }
        if grpc && response.headers().get("grpc-status").and_then(|v| v.to_str().ok()) != Some("4") {
            return Err(format!("{path}: missing gRPC DEADLINE_EXCEEDED"));
        }
        response
            .bytes()
            .await
            .map_err(|e| format!("{path}: response body: {e}"))?;
        if elapsed < Duration::from_millis(150) || elapsed > Duration::from_secs(2) {
            return Err(format!(
                "{path}: expected ~300ms establishment deadline, took {elapsed:?}"
            ));
        }
        backend
            .join_next()
            .await
            .ok_or_else(|| "missing TLS-stall task".to_string())?
            .map_err(|e| format!("TLS-stall task failed: {e}"))??;

        let entry = ctx
            .access_log_client()
            .get_access_log_with_retry(&trace_id, 10, 200)
            .await
            .map_err(|e| format!("{path}: access log: {e}"))?;
        if !entry.has_diagnostic("error", diagnostic) || entry.upstream_count() != 1 {
            return Err(format!(
                "{path}: expected {diagnostic} and exactly one attempt: {}",
                entry.data
            ));
        }
    }
    Ok(())
}
