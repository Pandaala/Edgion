// TCP relay idle-timeout suite.
//
// Verifies the shared L4 relay (`bidirectional_relay`) honors
// `tcpTimeout.idleTimeout` (baked to "2s" in the base EdgionGatewayConfig):
// - a stalled connection is closed shortly after the window;
// - a connection making periodic progress is NOT severed.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TcpIdleTimeoutTestSuite;

impl TcpIdleTimeoutTestSuite {
    fn test_idle_connection_closed() -> TestCase {
        TestCase::new(
            "tcp_idle_connection_closed",
            "Idle TCP connection is closed after the idle window",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut stream = match TcpStream::connect(&ctx.tcp_addr()).await {
                        Ok(s) => s,
                        Err(e) => return TestResult::failed(start.elapsed(), e.to_string()),
                    };
                    let mut buf = [0u8; 16];
                    match tokio::time::timeout(Duration::from_secs(6), stream.read(&mut buf)).await {
                        Ok(Ok(0)) => {
                            let waited = start.elapsed();
                            if waited >= Duration::from_secs(2) {
                                TestResult::passed_with_message(
                                    waited,
                                    format!("relay closed idle conn after {:?}", waited),
                                )
                            } else {
                                TestResult::failed(waited, format!("closed too early: {:?}", waited))
                            }
                        }
                        Ok(Ok(n)) => TestResult::failed(
                            start.elapsed(),
                            format!("unexpected {} bytes", n),
                        ),
                        Ok(Err(e)) => {
                            let waited = start.elapsed();
                            if waited >= Duration::from_secs(2) {
                                TestResult::passed_with_message(waited, format!("closed via err after {waited:?}: {e}"))
                            } else {
                                TestResult::failed(waited, format!("closed too early via err ({e}): {waited:?}"))
                            }
                        }
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "idle conn not closed within 6s".to_string(),
                        ),
                    }
                })
            },
        )
    }

    fn test_active_connection_survives() -> TestCase {
        TestCase::new(
            "tcp_active_connection_survives",
            "Active TCP connection is not severed by the idle timeout",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut stream = match TcpStream::connect(&ctx.tcp_addr()).await {
                        Ok(s) => s,
                        Err(e) => return TestResult::failed(start.elapsed(), e.to_string()),
                    };
                    for _ in 0..4 {
                        if let Err(e) = stream.write_all(b"x").await {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("write failed early: {}", e),
                            );
                        }
                        let mut buf = [0u8; 16];
                        let _ =
                            tokio::time::timeout(Duration::from_millis(200), stream.read(&mut buf))
                                .await;
                        tokio::time::sleep(Duration::from_millis(700)).await;
                    }
                    match stream.write_all(b"x").await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "active conn survived".to_string(),
                        ),
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("severed while active: {}", e),
                        ),
                    }
                })
            },
        )
    }
}

impl TestSuite for TcpIdleTimeoutTestSuite {
    fn name(&self) -> &str {
        "tcp_idle_timeout"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_idle_connection_closed(),
            Self::test_active_connection_survives(),
        ]
    }
}
