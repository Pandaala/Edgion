// TCPRoute DeniedBackendRef suite: a cross-namespace Service backendRef
// without a ReferenceGrant must fail closed after accept and never echo.
//
// Against edgion-tests/integration/conf/TCPRoute/DeniedBackendRef/:
// - 31954 (tcp): TCPRoute in edgion-test -> Service backend/tcp-denied-ref
//
// The backend EndpointSlice is real (test_server :30010). If the post-select
// gate is missing, the client receives an echo and the case fails.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TcpDeniedBackendRefTestSuite;

impl TcpDeniedBackendRefTestSuite {
    fn test_denied_ref_does_not_dial() -> TestCase {
        TestCase::new(
            "denied_cross_ns_ref_fail_closed",
            "TCPRoute cross-ns backendRef without ReferenceGrant must accept then close without echo",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = ctx.tcp_addr();
                    let payload = b"must-not-echo";

                    let mut stream = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await
                    {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => return TestResult::failed(start.elapsed(), format!("TCP connect failed: {e}")),
                        Err(_) => return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string()),
                    };

                    // The gateway may fail-close before the client write completes.
                    // BrokenPipe / reset is the authorized miss, not a harness error.
                    if stream.write_all(payload).await.is_err() {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            "Denied backendRef closed during write".to_string(),
                        );
                    }

                    let mut buf = vec![0u8; 64];
                    match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                        Ok(Ok(0)) | Ok(Err(_)) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Denied backendRef closed the connection without echoing".to_string(),
                        ),
                        Ok(Ok(n)) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected fail-close but received {n} bytes (gate missing?)"),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Read timed out; expected immediate fail-close".to_string(),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TcpDeniedBackendRefTestSuite {
    fn name(&self) -> &str {
        "TCPRoute DeniedBackendRef"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_denied_ref_does_not_dial()]
    }
}
