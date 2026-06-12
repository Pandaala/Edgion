// TCPRoute EdgionStreamPlugins hot-reload dynamic-scenario phase suites.
//
// Same listener/port across all phases; only the EdgionStreamPlugins allow-list
// flaps between phases:
//   allow phase : ESP allows 127.0.0.0/8 -> TCP echo succeeds
//   deny  phase : ESP allow-list removed -> connection denied (EOF/refused)
//
// The static counterpart is `TcpStreamPluginsTestSuite`; this suite reuses the
// same assertion shapes on a single dedicated port for the reload lifecycle.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Dedicated listener port for the ESP reload scenario
/// (ports.json suite key "TCPRoute/StreamPlugins/Dynamic").
const PORT: u16 = 31388;

/// Allow phase: the ESP allow-list permits localhost, so TCP echo must succeed.
pub struct TcpEspReloadAllowTestSuite;

impl TcpEspReloadAllowTestSuite {
    fn test_allow_echo() -> TestCase {
        TestCase::new(
            "tcp_esp_reload_allow_echo",
            "ESP allow-list present: localhost TCP echo should succeed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, PORT);
                    let test_data = b"Hello EspReload";

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }
                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await {
                                Ok(Ok(n)) if n > 0 && buf[..n] == test_data[..] => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "TCP echo succeeded — ESP allow-list permitted localhost".to_string(),
                                ),
                                Ok(Ok(n)) if n > 0 => {
                                    TestResult::failed(start.elapsed(), format!("Echo data mismatch ({} bytes)", n))
                                }
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection closed (0 bytes) — ESP may be denying".to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Read failed: {} — ESP may be denying", e),
                                ),
                                Err(_) => TestResult::failed(start.elapsed(), "Read timed out".to_string()),
                            }
                        }
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!("Connection refused (should be allowed): {}", e),
                        ),
                        Err(_) => {
                            TestResult::failed(start.elapsed(), "Connection timed out (should be allowed)".to_string())
                        }
                    }
                })
            },
        )
    }
}

impl TestSuite for TcpEspReloadAllowTestSuite {
    fn name(&self) -> &str {
        "TcpEspReloadAllow"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_allow_echo()]
    }
}

/// Deny phase: the ESP allow-list is removed, so the connection must be denied.
pub struct TcpEspReloadDenyTestSuite;

impl TcpEspReloadDenyTestSuite {
    fn test_deny_connection() -> TestCase {
        TestCase::new(
            "tcp_esp_reload_deny_connection",
            "ESP allow-list removed: connection should be denied",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, PORT);

                    match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            let _ = stream.write_all(b"Hello").await;
                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                                Ok(Ok(0)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection closed by ESP (EOF) — deny working".to_string(),
                                ),
                                Ok(Ok(n)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected denial but received {} bytes — ESP not denying", n),
                                ),
                                Ok(Err(_)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection rejected by ESP (read error)".to_string(),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out — ESP did not deny the connection".to_string(),
                                ),
                            }
                        }
                        Ok(Err(_)) => {
                            TestResult::passed_with_message(start.elapsed(), "Connection refused by ESP".to_string())
                        }
                        Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Connection dropped by ESP (timeout)".to_string(),
                        ),
                    }
                })
            },
        )
    }
}

impl TestSuite for TcpEspReloadDenyTestSuite {
    fn name(&self) -> &str {
        "TcpEspReloadDeny"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_deny_connection()]
    }
}
