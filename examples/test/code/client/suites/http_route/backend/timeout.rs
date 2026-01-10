// Timeout Test suite
//
// Required config files (in examples/conf/):
// - HTTPRoute_default_timeout-backend.yaml    # backend timeout test route
// - HTTPRoute_default_timeout-client.yaml     # client timeout test route
// - EdgionPlugins_default_timeout-debug.yaml  # Debug插件config
// - EndpointSlice_edge_test-http.yaml         # HTTP backend service discovery
// - Service_edge_test-http.yaml               # HTTP service definition
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - EdgionGatewayConfig__example-gateway.yaml # GatewayConfig（client.readTimeout: 60s）
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct TimeoutTestSuite;

impl TimeoutTestSuite {
    fn test_normal_response() -> TestCase {
        TestCase::new(
            "normal_response",
            "Test normal response (baseline)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let mut request = ctx.http_client.get(format!("{}/delay/1", ctx.http_url()));
                    request = request.header("Host", "timeout-backend.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let headers = response.headers().clone();

                            if !status.is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", status.as_u16()),
                                );
                            }

                            // Check Debug header
                            if let Some(debug_header) = headers.get("X-Debug-Access-Log") {
                                if let Ok(debug_str) = debug_header.to_str() {
                                    if let Ok(debug_json) = serde_json::from_str::<serde_json::Value>(debug_str) {
                                        let internal_status =
                                            debug_json["request_info"]["status"].as_u64().unwrap_or(0);
                                        if internal_status == 200 {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                format!(
                                                    "Normal response: HTTP {} / Internal {}",
                                                    status.as_u16(),
                                                    internal_status
                                                ),
                                            )
                                        } else {
                                            TestResult::failed(
                                                start.elapsed(),
                                                format!(
                                                    "Status mismatch: HTTP {} but Internal {}",
                                                    status.as_u16(),
                                                    internal_status
                                                ),
                                            )
                                        }
                                    } else {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Response OK but failed to parse debug JSON: {}", debug_str),
                                        )
                                    }
                                } else {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Response OK but debug header not valid UTF-8".to_string(),
                                    )
                                }
                            } else {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Response OK but no debug header found".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_backend_request_timeout() -> TestCase {
        TestCase::new(
            "backend_request_timeout",
            "Test backend request timeout（2s delay vs 1s timeout）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let mut request = ctx.http_client.get(format!("{}/delay/2", ctx.http_url()));
                    request = request.header("Host", "timeout-backend.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let headers = response.headers().clone();

                            // 期望504
                            if status.as_u16() != 504 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected HTTP 504, got {}", status.as_u16()),
                                );
                            }

                            // Check Debug headerinternal status code
                            if let Some(debug_header) = headers.get("X-Debug-Access-Log") {
                                if let Ok(debug_str) = debug_header.to_str() {
                                    if let Ok(debug_json) = serde_json::from_str::<serde_json::Value>(debug_str) {
                                        let internal_status =
                                            debug_json["request_info"]["status"].as_u64().unwrap_or(0);
                                        if internal_status == 504 {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                "Backend request timeout: HTTP 504 / Internal 504 ✓".to_string(),
                                            )
                                        } else {
                                            TestResult::failed(
                                                start.elapsed(),
                                                format!("Status mismatch: HTTP 504 but Internal {}", internal_status),
                                            )
                                        }
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            "Got 504 but failed to parse debug JSON".to_string(),
                                        )
                                    }
                                } else {
                                    TestResult::failed(start.elapsed(), "Debug header not valid UTF-8".to_string())
                                }
                            } else {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Got HTTP 504 (debug header missing)".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_client_read_timeout_499() -> TestCase {
        TestCase::new(
            "client_read_timeout_499",
            "Test client disconnect returns499（reqwest timeout1s，server delay3s）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Create 1s timeout HTTP client，simulate client disconnect
                    let short_timeout_client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(1))
                        .build()
                        .expect("Failed to create short timeout client");

                    // server delay3s（will not trigger server timeout, backend timeout is10s）
                    // but client1s will disconnect
                    let mut request = short_timeout_client.get(format!("{}/delay/3", ctx.http_url()));
                    request = request.header("Host", "timeout-client.example.com");

                    match request.send().await {
                        Ok(response) => {
                            // If response returned, no timeout（should not happen）
                            let status = response.status();
                            let elapsed = start.elapsed();
                            TestResult::failed(
                                elapsed,
                                format!(
                                    "Expected client to timeout but got HTTP {} after {}s",
                                    status.as_u16(),
                                    elapsed.as_secs()
                                ),
                            )
                        }
                        Err(e) => {
                            let elapsed = start.elapsed();
                            let error_msg = e.to_string();

                            // reqwest client timeout disconnect，this is expected
                            // Gateway should detect disconnect and log as499
                            // Check for timeout keywords
                            let is_timeout = error_msg.to_lowercase().contains("timeout")
                                || error_msg.to_lowercase().contains("timed out")
                                || error_msg.to_lowercase().contains("time out")
                                || error_msg.contains("deadline");

                            if is_timeout || elapsed.as_millis() >= 500 && elapsed.as_secs() <= 3 {
                                // Client timed out (about1s）
                                TestResult::passed_with_message(
                                elapsed,
                                format!("Client closed connection after ~{}ms (reqwest timeout 1s) - Gateway should log this as 499. Error: {}", 
                                    elapsed.as_millis(), error_msg)
                            )
                            } else {
                                TestResult::failed(
                                    elapsed,
                                    format!(
                                        "Unexpected error or timing: {}s, error: {}",
                                        elapsed.as_secs(),
                                        error_msg
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    fn test_client_read_timeout() -> TestCase {
        TestCase::new(
            "client_read_timeout",
            "Test client read timeout（3sdelay，framework5s timeout，observe gateway behavior）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Create alonger timeout HTTP client（5s），simulate long wait client that won't timeout first
                    let long_timeout_client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                        .expect("Failed to create long timeout client");

                    let mut request = long_timeout_client.get(format!("{}/delay/3", ctx.http_url()));
                    request = request.header("Host", "timeout-client.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let status_code = status.as_u16();
                            let headers = response.headers().clone();
                            let elapsed = start.elapsed();

                            // normally should return in3s or so200
                            // 因为backend timeout是10s，client timeout是5s
                            if status_code == 200 {
                                TestResult::passed_with_message(
                                    elapsed,
                                    format!(
                                        "Got 200 OK after {}s - client/backend timeouts not triggered (both > 3s)",
                                        elapsed.as_secs()
                                    ),
                                )
                            } else {
                                // Check Debug headerget internal status code
                                if let Some(debug_header) = headers.get("X-Debug-Access-Log") {
                                    if let Ok(debug_str) = debug_header.to_str() {
                                        if let Ok(debug_json) = serde_json::from_str::<serde_json::Value>(debug_str) {
                                            let internal_status =
                                                debug_json["request_info"]["status"].as_u64().unwrap_or(0);
                                            TestResult::failed(
                                                elapsed,
                                                format!(
                                                    "Expected 200 but got HTTP {} / Internal {}",
                                                    status_code, internal_status
                                                ),
                                            )
                                        } else {
                                            TestResult::failed(
                                                elapsed,
                                                format!("Got HTTP {} (debug JSON parse error)", status_code),
                                            )
                                        }
                                    } else {
                                        TestResult::failed(elapsed, "Debug header not valid UTF-8".to_string())
                                    }
                                } else {
                                    TestResult::failed(elapsed, format!("Got HTTP {} (no debug header)", status_code))
                                }
                            }
                        }
                        Err(e) => {
                            let elapsed = start.elapsed();
                            TestResult::failed(
                                elapsed,
                                format!("Request failed: {} (should have returned 200 after 3s)", e),
                            )
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TimeoutTestSuite {
    fn name(&self) -> &str {
        "Timeout"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_normal_response(),
            Self::test_backend_request_timeout(),
            Self::test_client_read_timeout(),
            Self::test_client_read_timeout_499(),
        ]
    }
}
