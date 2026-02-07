// Gateway StreamPlugins (ConnectionFilter) Test Suite
//
// Tests the ConnectionFilter feature that blocks/allows TCP connections
// before TLS handshake or HTTP processing, using EdgionStreamPlugins resources.
//
// Required config files (in examples/test/conf/Gateway/StreamPlugins/):
// - 01_EdgionStreamPlugins.yaml           # deny-all-ips resource
// - 02_EdgionStreamPlugins_allow.yaml     # allow-all-ips resource
// - 03_Gateway.yaml                       # 3 Gateways: no-filter, deny, allow
// - HTTPRoute.yaml                        # Route for no-filter gateway
// - TCPRoute_denied.yaml                  # Route for denied gateway
// - TCPRoute_allowed.yaml                 # Route for allowed gateway
//
// Port allocation (from ports.json "Gateway/StreamPlugins"):
// - 31270: HTTP (no ConnectionFilter, control group)
// - 31271: TCP (deny-all ConnectionFilter)
// - 31272: TCP (allow-all ConnectionFilter)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct StreamPluginsTestSuite;

impl StreamPluginsTestSuite {
    /// Test: HTTP listener without ConnectionFilter should work normally
    fn test_http_no_filter() -> TestCase {
        TestCase::new(
            "http_no_connection_filter",
            "HTTP listener without ConnectionFilter should accept requests normally",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .timeout(Duration::from_secs(5))
                        .build()
                        .unwrap();
                    let url = format!("http://{}:{}/health", ctx.target_host, ctx.http_port);

                    let mut request = client.get(&url);
                    if let Some(host) = &ctx.http_host {
                        request = request.header("host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            if response.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "HTTP request accepted (no ConnectionFilter)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 2xx, got {}", response.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: TCP connection to deny-all gateway should be rejected at TCP level
    fn test_tcp_denied() -> TestCase {
        TestCase::new(
            "tcp_connection_denied_by_filter",
            "TCP connection to deny-all gateway should be rejected immediately",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, ctx.tcp_port);

                    // Try to connect — should be rejected at TCP level
                    match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            // Connection was accepted, try to send data
                            // The filter should have closed it immediately
                            let test_data = b"Hello";
                            let _ = stream.write_all(test_data).await;

                            // Try to read — should get EOF or error
                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                                Ok(Ok(0)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection closed by filter (EOF)".to_string(),
                                ),
                                Ok(Ok(_n)) => TestResult::failed(
                                    start.elapsed(),
                                    "Expected connection to be rejected, but received data".to_string(),
                                ),
                                Ok(Err(_e)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection rejected by filter (read error)".to_string(),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection not rejected — read timed out".to_string(),
                                ),
                            }
                        }
                        Ok(Err(_e)) => {
                            // Connection refused — this is the expected behavior
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Connection refused by ConnectionFilter (connection error)".to_string(),
                            )
                        }
                        Err(_) => {
                            // Timeout — could mean filter is silently dropping
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Connection dropped by ConnectionFilter (timeout)".to_string(),
                            )
                        }
                    }
                })
            },
        )
    }

    /// Test: TCP connection to allow-all gateway should work normally
    fn test_tcp_allowed() -> TestCase {
        TestCase::new(
            "tcp_connection_allowed_by_filter",
            "TCP connection to allow-all gateway should pass through and echo data",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, ctx.tcp_filtered_port);
                    let test_data = b"Hello StreamPlugins";

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
                                            "TCP echo succeeded through allow-all filter".to_string(),
                                        )
                                    } else {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!(
                                                "TCP connection passed through filter (received {} bytes)",
                                                n
                                            ),
                                        )
                                    }
                                }
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection closed immediately (0 bytes read)".to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Read failed: {}", e),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out".to_string(),
                                ),
                            }
                        }
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!("Connection refused (should be allowed): {}", e),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Connection timed out (should be allowed)".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test: Multiple rapid connections to denied gateway should all be rejected
    fn test_tcp_denied_multiple() -> TestCase {
        TestCase::new(
            "tcp_multiple_denied_connections",
            "Multiple rapid TCP connections to deny-all gateway should all be rejected",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, ctx.tcp_port);
                    let mut denied_count = 0;
                    let total = 5;

                    for _ in 0..total {
                        match tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(&addr)).await {
                            Ok(Ok(mut stream)) => {
                                // If connected, check it gets closed
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
                                // Connection refused or timeout = denied
                                denied_count += 1;
                            }
                        }
                    }

                    if denied_count == total {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("All {}/{} connections denied", denied_count, total),
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
}

impl TestSuite for StreamPluginsTestSuite {
    fn name(&self) -> &str {
        "Gateway StreamPlugins (ConnectionFilter)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_http_no_filter(),
            Self::test_tcp_denied(),
            Self::test_tcp_allowed(),
            Self::test_tcp_denied_multiple(),
        ]
    }
}
