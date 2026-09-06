// UDPRoute DeniedBackendRef suite: a cross-namespace Service backendRef
// without a ReferenceGrant must drop the datagram (no echo).
//
// Against edgion-tests/integration/conf/UDPRoute/DeniedBackendRef/:
// - 31955 (udp): UDPRoute in edgion-test -> Service backend/udp-denied-ref
//
// The backend EndpointSlice is real (test_server :30011). If the post-select
// gate is missing, the client receives an echo and the case fails.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

pub struct UdpDeniedBackendRefTestSuite;

impl UdpDeniedBackendRefTestSuite {
    fn test_denied_ref_does_not_echo() -> TestCase {
        TestCase::new(
            "denied_cross_ns_ref_fail_closed",
            "UDPRoute cross-ns backendRef without ReferenceGrant must drop the packet",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let payload = b"must-not-echo";

                    let socket = match UdpSocket::bind("127.0.0.1:0").await {
                        Ok(s) => s,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("UDP bind failed: {e}")),
                    };
                    if let Err(e) = socket.send_to(payload, &ctx.udp_addr()).await {
                        return TestResult::failed(start.elapsed(), format!("UDP send failed: {e}"));
                    }

                    let mut buf = vec![0u8; 64];
                    match tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf)).await {
                        Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Denied backendRef dropped the datagram".to_string(),
                        ),
                        Ok(Ok((n, _))) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected drop but received {n} bytes (gate missing?)"),
                        ),
                        Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("UDP recv failed: {e}")),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for UdpDeniedBackendRefTestSuite {
    fn name(&self) -> &str {
        "UDPRoute DeniedBackendRef"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_denied_ref_does_not_echo()]
    }
}
