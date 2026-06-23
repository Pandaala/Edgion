// TCP keepalive smoke test suite.
//
// Verifies that a TCPRoute carrying the three keepalive annotations
//   edgion.io/tcp-keepalive-time
//   edgion.io/tcp-keepalive-interval
//   edgion.io/tcp-keepalive-probes
// still proxies traffic end-to-end without regression.
//
// This is a BEHAVIORAL smoke test only.  It does NOT assert on kernel
// SO_KEEPALIVE socket options (no introspection hook exists; on macOS
// keepalive is a no-op stub).  The sole goal is: annotations accepted,
// route configured, TCP echo still works.
//
// Required config files (edgion-tests/integration/conf/TCPRoute/KeepaliveSmoke/):
//   Gateway.yaml
//   Service_test-tcp.yaml
//   EndpointSlice_test-tcp.yaml
//   TCPRoute.yaml          <- carries the three keepalive annotations
//   TCPRoute_filtered.yaml <- same annotations on the filtered listener

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TcpKeepaliveSmokeTestSuite;

impl TcpKeepaliveSmokeTestSuite {
    fn test_tcp_connection() -> TestCase {
        TestCase::new(
            "tcp_keepalive_smoke_connection",
            "TCP connection succeeds when keepalive annotations are present on the route",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match TcpStream::connect(&ctx.tcp_addr()).await {
                        Ok(_stream) => TestResult::passed_with_message(
                            start.elapsed(),
                            "TCP connection established (keepalive annotations accepted)".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                    }
                })
            },
        )
    }

    fn test_tcp_echo() -> TestCase {
        TestCase::new(
            "tcp_keepalive_smoke_echo",
            "TCP echo succeeds when keepalive annotations are present on the route",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_data = b"Hello TCP KeepaliveSmoke";

                    match TcpStream::connect(&ctx.tcp_addr()).await {
                        Ok(mut stream) => {
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), e.to_string());
                            }
                            let mut buffer = vec![0u8; 1024];
                            match stream.read(&mut buffer).await {
                                Ok(n) if n > 0 && &buffer[..n] == test_data => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "TCP echo matched (keepalive annotations did not break routing)".to_string(),
                                ),
                                Ok(_) => TestResult::failed(start.elapsed(), "Echo data mismatch".to_string()),
                                Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                    }
                })
            },
        )
    }

    fn test_tcp_filtered_echo() -> TestCase {
        TestCase::new(
            "tcp_keepalive_smoke_filtered_echo",
            "TCP echo on sectionName-filtered listener succeeds when keepalive annotations are present",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // This test is only meaningful in Gateway mode
                    if !ctx.gateway {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            "Skipped in Direct mode (Gateway only test)".to_string(),
                        );
                    }

                    let test_data = b"Hello TCP KeepaliveSmoke Filtered";

                    match TcpStream::connect(&ctx.tcp_filtered_addr()).await {
                        Ok(mut stream) => {
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), e.to_string());
                            }
                            let mut buffer = vec![0u8; 1024];
                            match stream.read(&mut buffer).await {
                                Ok(n) if n > 0 && &buffer[..n] == test_data => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "sectionName:tcp-filtered echo matched (keepalive annotations accepted)"
                                        .to_string(),
                                ),
                                Ok(_) => TestResult::failed(start.elapsed(), "Echo data mismatch".to_string()),
                                Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TcpKeepaliveSmokeTestSuite {
    fn name(&self) -> &str {
        "tcp_keepalive_smoke"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_tcp_connection(),
            Self::test_tcp_echo(),
            Self::test_tcp_filtered_echo(),
        ]
    }
}
