// TCP Test suite
//
// Required config files (in examples/conf/):
// - EndpointSlice_edge_test-tcp.yaml          # TCP backend service discovery
// - Service_edge_test-tcp.yaml                # TCP service definition
// - TCPRoute_edge_test-tcp.yaml               # TCP routing rules（listening port）
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TcpTestSuite;

impl TcpTestSuite {
    fn test_tcp_connection() -> TestCase {
        TestCase::new("tcp_connection", "Test TCP connection", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                match TcpStream::connect(&ctx.tcp_addr()).await {
                    Ok(_stream) => {
                        TestResult::passed_with_message(start.elapsed(), "TCP connection established".to_string())
                    }
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        })
    }

    fn test_tcp_echo() -> TestCase {
        TestCase::new("tcp_echo", "Test TCP echo", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let test_data = b"Hello TCP";

                match TcpStream::connect(&ctx.tcp_addr()).await {
                    Ok(mut stream) => {
                        // Send data
                        if let Err(e) = stream.write_all(test_data).await {
                            return TestResult::failed(start.elapsed(), e.to_string());
                        }

                        // Read response
                        let mut buffer = vec![0u8; 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                if n > 0 && &buffer[..n] == test_data {
                                    TestResult::passed(start.elapsed())
                                } else {
                                    TestResult::failed(start.elapsed(), "Echo data mismatch".to_string())
                                }
                            }
                            Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), e.to_string()),
                }
            })
        })
    }

    fn test_tcp_section_name_filtered() -> TestCase {
        TestCase::new(
            "tcp_section_name_filtered",
            "Test TCP sectionName match (tcp-filtered listener, Gateway mode only)",
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

                    let test_data = b"Hello TCP Filtered";

                    match TcpStream::connect(&ctx.tcp_filtered_addr()).await {
                        Ok(mut stream) => {
                            // Send data
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), e.to_string());
                            }

                            // Read response
                            let mut buffer = vec![0u8; 1024];
                            match stream.read(&mut buffer).await {
                                Ok(n) => {
                                    if n > 0 && &buffer[..n] == test_data {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "sectionName:tcp-filtered matched correctly".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(start.elapsed(), "Echo data mismatch".to_string())
                                    }
                                }
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
impl TestSuite for TcpTestSuite {
    fn name(&self) -> &str {
        "TCP"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_tcp_connection(),
            Self::test_tcp_echo(),
            Self::test_tcp_section_name_filtered(),
        ]
    }
}
