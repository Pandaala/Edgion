// HTTP Security Test suite
//
// Test HTTP request security validation，including:
// - Hostname validation (missing/empty)
//
// Required config files (in examples/conf/):
// - EndpointSlice_edge_test-http.yaml         # HTTP backend service discovery
// - Service_edge_test-http.yaml               # HTTP service definition
// - httproute_default_example-route.yaml      # HTTP routing rules（Host: test.example.com）
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct HttpSecurityTestSuite;

impl HttpSecurityTestSuite {
    fn test_missing_hostname() -> TestCase {
        TestCase::new(
            "missing_hostname",
            "Test missing Host header returns 400",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Extract host and port from HTTP URL
                    let url = ctx.http_url();
                    let parts: Vec<&str> = url.split("://").collect();
                    let addr = if parts.len() > 1 {
                        parts[1].to_string()
                    } else {
                        parts[0].to_string()
                    };

                    // Send raw HTTP request without Host header
                    match TcpStream::connect(&addr).await {
                        Ok(mut stream) => {
                            // Send HTTP/1.1 request without Host header
                            let request = "GET /health HTTP/1.1\r\nConnection: close\r\n\r\n";

                            if let Err(e) = stream.write_all(request.as_bytes()).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }

                            // Read response
                            let mut response = Vec::new();
                            if let Err(e) = stream.read_to_end(&mut response).await {
                                return TestResult::failed(start.elapsed(), format!("Read failed: {}", e));
                            }

                            let response_str = String::from_utf8_lossy(&response);

                            // Check for 400 Bad Request
                            if response_str.contains("HTTP/1.1 400") || response_str.contains("400 Bad Request") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Missing Host header rejected with 400".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 400 Bad Request, got: {}",
                                        response_str.lines().next().unwrap_or("")
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Connection failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_empty_hostname() -> TestCase {
        TestCase::new(
            "empty_hostname",
            "Test empty Host header returns 400",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Extract host and port from HTTP URL
                    let url = ctx.http_url();
                    let parts: Vec<&str> = url.split("://").collect();
                    let addr = if parts.len() > 1 {
                        parts[1].to_string()
                    } else {
                        parts[0].to_string()
                    };

                    // Send raw HTTP request with empty Host header
                    match TcpStream::connect(&addr).await {
                        Ok(mut stream) => {
                            // Send HTTP/1.1 request with empty Host header
                            let request = "GET /health HTTP/1.1\r\nHost: \r\nConnection: close\r\n\r\n";

                            if let Err(e) = stream.write_all(request.as_bytes()).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }

                            // Read response
                            let mut response = Vec::new();
                            if let Err(e) = stream.read_to_end(&mut response).await {
                                return TestResult::failed(start.elapsed(), format!("Read failed: {}", e));
                            }

                            let response_str = String::from_utf8_lossy(&response);

                            // Check for 400 Bad Request
                            if response_str.contains("HTTP/1.1 400") || response_str.contains("400 Bad Request") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Empty Host header rejected with 400".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 400 Bad Request, got: {}",
                                        response_str.lines().next().unwrap_or("")
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Connection failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_valid_hostname() -> TestCase {
        TestCase::new(
            "valid_hostname",
            "Test valid Host header processed normally",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Use reqwest for normal request with valid Host header
                    let mut request = ctx.http_client.get(format!("{}/health", ctx.http_url()));
                    if let Some(host) = &ctx.http_host {
                        request = request.header("Host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            if response.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Valid Host header accepted ({})", response.status()),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 2xx, got {}", response.status()))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_hostname_with_port() -> TestCase {
        TestCase::new(
            "hostname_with_port",
            "Test Host header with port",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Extract host and port from HTTP URL
                    let url = ctx.http_url();
                    let parts: Vec<&str> = url.split("://").collect();
                    let addr = if parts.len() > 1 {
                        parts[1].to_string()
                    } else {
                        parts[0].to_string()
                    };

                    // Send raw HTTP request with Host:Port format
                    match TcpStream::connect(&addr).await {
                        Ok(mut stream) => {
                            let host_with_port = ctx
                                .http_host
                                .as_ref()
                                .map(|h| format!("{}:8080", h))
                                .unwrap_or_else(|| "test.example.com:8080".to_string());

                            let request = format!(
                                "GET /health HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                                host_with_port
                            );

                            if let Err(e) = stream.write_all(request.as_bytes()).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }

                            // Read response
                            let mut response = Vec::new();
                            if let Err(e) = stream.read_to_end(&mut response).await {
                                return TestResult::failed(start.elapsed(), format!("Read failed: {}", e));
                            }

                            let response_str = String::from_utf8_lossy(&response);

                            // Should accept Host with port
                            if response_str.contains("HTTP/1.1 200") || response_str.contains("HTTP/1.1 404") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Host header with port accepted".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Unexpected response: {}", response_str.lines().next().unwrap_or("")),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Connection failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for HttpSecurityTestSuite {
    fn name(&self) -> &str {
        "HTTP Security"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_missing_hostname(),
            Self::test_empty_hostname(),
            Self::test_valid_hostname(),
            Self::test_hostname_with_port(),
        ]
    }
}
