// Security Protection Test suite
//
// Required config files (in examples/conf/):
// - EndpointSlice_edge_test-http.yaml         # HTTP backend service discovery
// - Service_edge_test-http.yaml               # HTTP service definition
// - httproute_default_example-route.yaml      # HTTP routing rules（Host: test.example.com）
//   Note: route contains maxXFFLength config for security
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct SecurityTestSuite;

impl SecurityTestSuite {
    fn test_normal_xff() -> TestCase {
        TestCase::new(
            "normal_xff_length",
            "Test normal length X-Forwarded-For (< 200 bytes)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let url = format!("{}/health", ctx.http_url());

                    // Normal X-Forwarded-For: ~50 bytes
                    let normal_xff = "203.0.113.1, 198.51.100.2, 192.168.1.1";

                    let mut request = client.get(&url).header("x-forwarded-for", normal_xff);

                    if let Some(host) = &ctx.http_host {
                        request = request.header("host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            if response.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Normal XFF accepted ({} bytes)", normal_xff.len()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK, got {}", response.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_excessive_xff() -> TestCase {
        TestCase::new(
            "excessive_xff_length",
            "Test oversized X-Forwarded-For (> 200 bytes)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let url = format!("{}/health", ctx.http_url());

                    // Generate XFF > 200 bytes
                    // Each IP "XXX.XXX.XXX.XXX" is up to 15 chars, plus ", " separator (2 chars)
                    // To exceed 200 bytes, we need at least 15-20 IPs
                    let mut xff_parts = Vec::new();
                    for i in 0..20 {
                        xff_parts.push(format!("203.0.{}.{}", i / 256, i % 256));
                    }
                    let long_xff = xff_parts.join(", ");

                    assert!(
                        long_xff.len() > 200,
                        "Test XFF should be > 200 bytes, got {}",
                        long_xff.len()
                    );

                    let mut request = client.get(&url).header("x-forwarded-for", &long_xff);

                    if let Some(host) = &ctx.http_host {
                        request = request.header("host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            if response.status() == 400 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Long XFF rejected with 400 ({} bytes)", long_xff.len()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 400 Bad Request, got {}", response.status()),
                                )
                            }
                        }
                        Err(e) => {
                            // Connection reset or similar errors are also acceptable for rejected requests
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("✓ Long XFF rejected (connection error: {})", e),
                            )
                        }
                    }
                })
            },
        )
    }

    fn test_boundary_xff() -> TestCase {
        TestCase::new(
            "boundary_xff_length",
            "Test boundary X-Forwarded-For (exactly 200 bytes)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let url = format!("{}/health", ctx.http_url());

                    // Generate XFF exactly 200 bytes
                    // Build a string with IP addresses until we reach exactly 200 bytes
                    let mut xff = String::new();
                    for i in 0..20 {
                        if !xff.is_empty() {
                            xff.push_str(", ");
                        }
                        xff.push_str(&format!("10.0.{}.{}", i / 256, i % 256));

                        if xff.len() >= 200 {
                            xff.truncate(200);
                            break;
                        }
                    }

                    assert_eq!(xff.len(), 200, "Test XFF should be exactly 200 bytes");

                    let mut request = client.get(&url).header("x-forwarded-for", &xff);

                    if let Some(host) = &ctx.http_host {
                        request = request.header("host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            if response.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Boundary XFF accepted (exactly 200 bytes)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK for boundary case, got {}", response.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_no_xff() -> TestCase {
        TestCase::new("no_xff_header", "Test no X-Forwarded-For header", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = reqwest::Client::builder().no_proxy().build().unwrap();
                let url = format!("{}/health", ctx.http_url());

                let mut request = client.get(&url);

                if let Some(host) = &ctx.http_host {
                    request = request.header("host", host);
                }

                match request.send().await {
                    Ok(response) => {
                        if response.status().is_success() {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "✓ Request without XFF accepted".to_string(),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), format!("Expected 200 OK, got {}", response.status()))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }
}

impl TestSuite for SecurityTestSuite {
    fn name(&self) -> &str {
        "Security Protection Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_normal_xff(),
            Self::test_excessive_xff(),
            Self::test_boundary_xff(),
            Self::test_no_xff(),
        ]
    }
}
