// TCPRoute StreamPlugins Test Suite
//
// Tests that stream plugins on TCPRoute annotations work correctly,
// especially verifying that real client IP is extracted (not hardcoded "0.0.0.0").
//
// Required config files (in examples/test/conf/TCPRoute/StreamPlugins/):
// - 01_EdgionStreamPlugins_deny.yaml          # deny-all resource
// - 02_EdgionStreamPlugins_allow_localhost.yaml # allow 127.0.0.0/8 only
// - 03_Gateway.yaml                           # Gateway with 2 TCP listeners
// - 04_TCPRoute_allow_localhost.yaml           # Route with allow-localhost plugin
// - 05_TCPRoute_deny.yaml                     # Route with deny-all plugin
// - Service_test-tcp.yaml / EndpointSlice_test-tcp.yaml
//
// Port allocation (from ports.json "TCPRoute/StreamPlugins"):
// - 31274 (tcp): allow-localhost filter (regression test for IP extraction)
// - 31275 (tcp_filtered): deny-all filter

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TcpStreamPluginsTestSuite;

impl TcpStreamPluginsTestSuite {
    /// Regression test: allow-localhost plugin should allow connections from 127.0.0.1
    ///
    /// The EdgionStreamPlugins resource has: defaultAction=deny, allowList=[127.0.0.0/8]
    /// - With old code (0.0.0.0): 0.0.0.0 NOT in 127.0.0.0/8 → DENY (bug)
    /// - With fixed code (127.0.0.1): 127.0.0.1 IS in 127.0.0.0/8 → ALLOW (correct)
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
                            // Send data
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Write failed: {}", e),
                                );
                            }

                            // Read echo response
                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await {
                                Ok(Ok(n)) if n > 0 => {
                                    if &buf[..n] == test_data {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "TCP echo succeeded — real IP (127.0.0.1) correctly matched allow-list".to_string(),
                                        )
                                    } else {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!(
                                                "TCP connection allowed by stream plugin (received {} bytes)",
                                                n
                                            ),
                                        )
                                    }
                                }
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection closed immediately (0 bytes) — stream plugin may be using wrong IP".to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Read failed: {} — stream plugin may have denied the connection", e),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out".to_string(),
                                ),
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
                                match tokio::time::timeout(
                                    Duration::from_secs(1),
                                    stream.read(&mut buf),
                                )
                                .await
                                {
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
                            format!(
                                "Only {}/{} connections denied (expected all)",
                                denied_count, total
                            ),
                        )
                    }
                })
            },
        )
    }

    /// Test: short name annotation format (no namespace prefix) should work
    /// The deny TCPRoute uses "tcp-deny-all" (without "edgion-test/" prefix)
    /// and namespace should be inferred from the TCPRoute's own namespace
    fn test_short_name_annotation() -> TestCase {
        TestCase::new(
            "tcp_stream_plugin_short_name_annotation",
            "Short name annotation (without namespace/) should correctly resolve EdgionStreamPlugins",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = ctx.tcp_filtered_addr();

                    // The deny TCPRoute uses short name format "tcp-deny-all"
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
                        Ok(Err(_)) | Err(_) => {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Short name annotation resolved correctly — connection refused/dropped".to_string(),
                            )
                        }
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
        ]
    }
}
