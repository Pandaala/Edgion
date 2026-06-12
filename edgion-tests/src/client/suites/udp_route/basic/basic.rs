// UDP Test suite
//
// Required config files (in examples/conf/):
// - EndpointSlice_edge_test-udp.yaml          # UDP backend service discovery
// - Service_edge_test-udp.yaml                # UDP service definition
// - UDPRoute_edge_test-udp.yaml               # UDP routing rules（listening port）
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;
use tokio::net::UdpSocket;

pub struct UdpTestSuite;

impl UdpTestSuite {
    fn test_udp_send_receive() -> TestCase {
        TestCase::new("udp_send_receive", "Test UDP send and receive", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let test_data = b"Hello UDP";

                match UdpSocket::bind("0.0.0.0:0").await {
                    Ok(socket) => {
                        // Send data
                        if let Err(e) = socket.send_to(test_data, &ctx.udp_addr()).await {
                            return TestResult::failed(start.elapsed(), e.to_string());
                        }

                        // Receive response
                        let mut buffer = vec![0u8; 1024];
                        match tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv_from(&mut buffer))
                            .await
                        {
                            Ok(Ok((n, _))) => {
                                if n > 0 && &buffer[..n] == test_data {
                                    TestResult::passed(start.elapsed())
                                } else {
                                    TestResult::failed(start.elapsed(), "Echo data mismatch".to_string())
                                }
                            }
                            Ok(Err(e)) => TestResult::failed(start.elapsed(), e.to_string()),
                            Err(_) => {
                                TestResult::failed(start.elapsed(), "Timeout waiting for UDP response".to_string())
                            }
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        })
    }
}

#[async_trait]
impl TestSuite for UdpTestSuite {
    fn name(&self) -> &str {
        "UDP"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_udp_send_receive()]
    }
}
