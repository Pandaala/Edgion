// TCPRoute StreamPlugins Test Suite
//
// Tests that stream plugins on TCPRoute annotations work correctly,
// especially verifying that real client IP is extracted (not hardcoded "0.0.0.0").
//
// Required config files (in edgion-tests/integration/conf/TCPRoute/StreamPlugins/):
// - 01_EdgionStreamPlugins_deny.yaml          # deny-all resource
// - 02_EdgionStreamPlugins_allow_localhost.yaml # allow 127.0.0.0/8 only
// - 03_Gateway.yaml                           # Gateway with 2 TCP listeners
// - 04_TCPRoute_allow_localhost.yaml           # Route with allow-localhost plugin
// - 05_TCPRoute_deny.yaml                     # Route with deny-all plugin
// - Service_test-tcp.yaml / EndpointSlice_test-tcp.yaml
//
// Port allocation (from ports.json "TCPRoute/StreamPlugins"):
// - 31274 (tcp): allow-localhost filter (guards real client IP extraction)
// - 31275 (tcp_filtered): deny-all filter
// - 31273 (tcp_fail_close): TCPRoute references missing ESP (fail-close contract)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::edgion_backend::gateway_admin;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TcpStreamPluginsTestSuite;

impl TcpStreamPluginsTestSuite {
    /// Validate the Gateway's pool before testing healthy-Redis admission.
    /// Container port forwarding can retire idle connections during earlier
    /// suites. Probe every fixed member without consuming the connection-rate
    /// budget or replaying traffic.
    async fn wait_for_redis_fixture(ctx: &TestContext) -> Result<(), String> {
        let fixture: serde_yaml::Value = serde_yaml::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/LinkSys/Redis/01_LinkSys_default_redis-test.yaml"
        )))
        .map_err(|_| "Invalid Redis fixture YAML".to_string())?;
        let pool_size = fixture["spec"]["config"]["pool"]["size"]
            .as_u64()
            .filter(|size| (1..=64).contains(size))
            .ok_or_else(|| "Redis fixture must declare a valid pool size".to_string())?;
        let url = format!(
            "{}/api/v1/testing/link-sys/redis/default%2Fredis-test/ping?all_members=true",
            gateway_admin::admin_base(ctx)
        );
        let ready = async {
            loop {
                let response = gateway_admin::authed(ctx.http_client.get(&url).timeout(Duration::from_secs(1)))
                    .send()
                    .await;
                let healthy = match response {
                    Ok(response) if response.status().is_success() => {
                        response.json::<serde_json::Value>().await.is_ok_and(|body| {
                            body["success"] == true
                                && body["data"]["healthy"] == true
                                && body["data"]["checked_members"] == pool_size
                        })
                    }
                    _ => false,
                };
                if healthy {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), ready)
            .await
            .map_err(|_| "Gateway Redis fixture did not become ready within 10s".to_string())
    }

    /// Tests allow-localhost plugin: verifies the real client IP (127.0.0.1) is
    /// extracted and correctly matched against the allowList.
    ///
    /// Config: defaultAction=deny, allowList=[127.0.0.0/8]
    /// 127.0.0.1 IS in 127.0.0.0/8 — connection must be ALLOWED.
    fn test_allow_localhost_echo() -> TestCase {
        TestCase::new(
            "tcp_stream_plugin_allow_localhost",
            "TCPRoute stream plugin should allow localhost connection (IP extraction regression test)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = ctx.tcp_addr();
                    let test_data = b"Hello StreamPlugin";

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }

                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await {
                                Ok(Ok(n)) if n > 0 => {
                                    if &buf[..n] == test_data {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "TCP echo succeeded — real IP (127.0.0.1) correctly matched allow-list"
                                                .to_string(),
                                        )
                                    } else {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("TCP connection allowed by stream plugin (received {} bytes)", n),
                                        )
                                    }
                                }
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection closed immediately (0 bytes) — stream plugin may be using wrong IP"
                                        .to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Read failed: {} — stream plugin may have denied the connection", e),
                                ),
                                Err(_) => TestResult::failed(start.elapsed(), "Read timed out".to_string()),
                            }
                        }
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!("Connection refused (should be allowed for localhost): {}", e),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Connection timed out (should be allowed for localhost)".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test: deny-all stream plugin should reject TCP connections at application level
    fn test_deny_all_connection() -> TestCase {
        TestCase::new(
            "tcp_stream_plugin_deny_all",
            "TCPRoute stream plugin with deny-all should reject connection",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = ctx.tcp_filtered_addr();

                    // Connect — TCP handshake may succeed (stream plugin runs after accept),
                    // but the connection should be closed immediately without proxying data
                    match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            // Connection accepted at TCP level, try to send data
                            let test_data = b"Hello";
                            let _ = stream.write_all(test_data).await;

                            // Try to read — should get EOF or error (no proxy to backend)
                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                                Ok(Ok(0)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection closed by stream plugin (EOF) — deny working".to_string(),
                                ),
                                Ok(Ok(n)) => {
                                    // Got data back — this means the stream plugin didn't deny
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!("Expected denial but received {} bytes — stream plugin not working", n),
                                    )
                                }
                                Ok(Err(_e)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection rejected by stream plugin (read error)".to_string(),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out — stream plugin did not deny the connection".to_string(),
                                ),
                            }
                        }
                        Ok(Err(_e)) => {
                            // Connection refused at TCP level — also acceptable for denial
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Connection refused by stream plugin".to_string(),
                            )
                        }
                        Err(_) => {
                            // Timeout — could mean filter is silently dropping
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Connection dropped by stream plugin (timeout)".to_string(),
                            )
                        }
                    }
                })
            },
        )
    }

    /// Test: multiple connections to deny-all should all be rejected
    fn test_deny_all_multiple() -> TestCase {
        TestCase::new(
            "tcp_stream_plugin_deny_all_multiple",
            "Multiple TCP connections to deny-all TCPRoute should all be rejected",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = ctx.tcp_filtered_addr();
                    let mut denied_count = 0;
                    let total = 5;

                    for _ in 0..total {
                        match tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(&addr)).await {
                            Ok(Ok(mut stream)) => {
                                let mut buf = vec![0u8; 16];
                                match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf)).await {
                                    Ok(Ok(0)) | Ok(Err(_)) => denied_count += 1,
                                    Err(_) => {} // timeout, not clearly denied
                                    _ => {}
                                }
                            }
                            Ok(Err(_)) | Err(_) => {
                                denied_count += 1;
                            }
                        }
                    }

                    if denied_count == total {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("All {}/{} connections denied by stream plugin", denied_count, total),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Only {}/{} connections denied (expected all)", denied_count, total),
                        )
                    }
                })
            },
        )
    }

    /// Test: short name annotation format (no namespace prefix) should work.
    /// The deny TCPRoute uses "tcp-deny-all" (without "edgion-test/" prefix);
    /// namespace is inferred from the TCPRoute's own namespace.
    fn test_short_name_annotation() -> TestCase {
        TestCase::new(
            "tcp_stream_plugin_short_name_annotation",
            "Short name annotation (without namespace/) should correctly resolve EdgionStreamPlugins",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = ctx.tcp_filtered_addr();

                    // If annotation parsing works, connection should be denied
                    match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            let mut buf = vec![0u8; 16];
                            match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                                Ok(Ok(0)) | Ok(Err(_)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Short name annotation resolved correctly — connection denied".to_string(),
                                ),
                                Ok(Ok(n)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected denial but received {} bytes — short name annotation may not be resolved", n),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out — short name annotation may not be resolved".to_string(),
                                ),
                            }
                        }
                        Ok(Err(_)) | Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Short name annotation resolved correctly — connection refused/dropped".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test: ConnectionRateLimit local-only gate (no Redis) admits up to `rate`
    /// connections per interval and rejects excess ones at L4.
    ///
    /// Config: perListener { rate: 2, interval: "1s" }
    /// We open 5 connections in a tight sequential loop.  The local token-bucket
    /// starts full (2 tokens); subsequent connects within the same second run out
    /// of tokens and must be rejected (same accept-then-drop path as deny-all).
    /// Assertion: admitted <= 2 AND rejected >= 1.
    fn test_conn_rate_limit_local() -> TestCase {
        TestCase::new(
            "tcp_stream_plugin_conn_rate_limit_local",
            "ConnectionRateLimit (local, no Redis): at most 2 connections admitted per 1s window",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, 31277);
                    let total = 5;
                    let mut admitted = 0usize;
                    let mut rejected = 0usize;

                    for _ in 0..total {
                        match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                            Ok(Ok(mut stream)) => {
                                // Connection accepted at TCP level; try to send data and read.
                                // If the rate gate admitted this connection the backend will echo.
                                // If rejected, the gateway drops the connection after accept —
                                // same path as deny-all: we see EOF (read returns 0) or a read error.
                                let test_data = b"Hello";
                                let _ = stream.write_all(test_data).await;
                                let mut buf = vec![0u8; 1024];
                                match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                                    Ok(Ok(0)) | Ok(Err(_)) => {
                                        // EOF or read error — rate gate closed this connection.
                                        rejected += 1;
                                    }
                                    Ok(Ok(_n)) => {
                                        // Got data back — connection was admitted and proxied.
                                        admitted += 1;
                                    }
                                    Err(_) => {
                                        // Read timed out — not conclusive; treat as neither.
                                    }
                                }
                            }
                            Ok(Err(_)) | Err(_) => {
                                // Refused or timed out at TCP level — also a rejection.
                                rejected += 1;
                            }
                        }
                    }

                    if admitted <= 2 && rejected >= 1 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "ConnectionRateLimit local gate working: admitted={} (≤2), rejected={} (≥1)",
                                admitted, rejected
                            ),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "ConnectionRateLimit local gate misbehaving: admitted={} rejected={} (expected admitted<=2, rejected>=1)",
                                admitted, rejected
                            ),
                        )
                    }
                })
            },
        )
    }

    /// Test: ConnectionRateLimit Redis-backed perSourceIp dimension limits
    /// connections from one source IP across the fleet via Redis. perSourceIp
    /// has no local backstop, so every connection goes through the Redis sliding
    /// window. Config: perSourceIp { rate: 3, interval: "5s" } on a real Redis.
    /// We open 6 connections from localhost (one source key) within the window;
    /// assert admitted <= 3 AND rejected >= 1. The 5s window keeps the 6
    /// sequential connects safely inside one window even on a slow-but-healthy Redis.
    fn test_conn_rate_limit_redis() -> TestCase {
        TestCase::new(
            "tcp_stream_plugin_conn_rate_limit_redis",
            "ConnectionRateLimit (Redis perSourceIp): at most 3 connections admitted per 5s window",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = Self::wait_for_redis_fixture(&ctx).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    let addr = format!("{}:{}", ctx.target_host, 31278);
                    let total = 6;
                    let mut admitted = 0usize;
                    let mut rejected = 0usize;

                    for _ in 0..total {
                        match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                            Ok(Ok(mut stream)) => {
                                let test_data = b"Hello";
                                let _ = stream.write_all(test_data).await;
                                let mut buf = vec![0u8; 1024];
                                match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                                    Ok(Ok(0)) | Ok(Err(_)) => rejected += 1,
                                    Ok(Ok(_n)) => admitted += 1,
                                    Err(_) => {}
                                }
                            }
                            Ok(Err(_)) | Err(_) => rejected += 1,
                        }
                    }

                    if admitted <= 3 && rejected >= 1 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "ConnectionRateLimit Redis perSourceIp working: admitted={} (≤3), rejected={} (≥1)",
                                admitted, rejected
                            ),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "ConnectionRateLimit Redis perSourceIp misbehaving: admitted={} rejected={} (expected admitted<=3, rejected>=1)",
                                admitted, rejected
                            ),
                        )
                    }
                })
            },
        )
    }

    /// Test: TCPRoute referencing a non-existent EdgionStreamPlugins must
    /// fail-close (reject the connection) rather than silently admit traffic.
    /// Locks in the fail-close contract introduced in the gateway TCP path.
    fn test_missing_esp_fail_close() -> TestCase {
        TestCase::new(
            "tcp_stream_plugin_missing_esp_fail_close",
            "TCPRoute referencing a non-existent EdgionStreamPlugins must fail-close",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Dedicated listener wired in 03_Gateway.yaml + 06_TCPRoute_fail_close.yaml.
                    // Port comes from ports.json TCPRoute/StreamPlugins.tcp_fail_close.
                    let addr = format!("{}:{}", ctx.target_host, 31273);
                    let test_data = b"Hello FailClose";

                    match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            // TCP handshake may succeed; the proxy rejects after accept.
                            let _ = stream.write_all(test_data).await;
                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                                Ok(Ok(0)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection closed by gateway (EOF) — fail-close on missing ESP".to_string(),
                                ),
                                Ok(Ok(n)) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected fail-close (no proxy) but received {} bytes — missing ESP was admitted",
                                        n
                                    ),
                                ),
                                Ok(Err(_e)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection rejected (read error) — fail-close on missing ESP".to_string(),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out — gateway did not fail-close on missing ESP".to_string(),
                                ),
                            }
                        }
                        Ok(Err(_)) | Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Connection refused/dropped — fail-close on missing ESP".to_string(),
                        ),
                    }
                })
            },
        )
    }
}

impl TestSuite for TcpStreamPluginsTestSuite {
    fn name(&self) -> &str {
        "TCPRoute StreamPlugins"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_allow_localhost_echo(),
            Self::test_deny_all_connection(),
            Self::test_deny_all_multiple(),
            Self::test_short_name_annotation(),
            Self::test_missing_esp_fail_close(),
            Self::test_conn_rate_limit_local(),
            Self::test_conn_rate_limit_redis(),
        ]
    }
}
