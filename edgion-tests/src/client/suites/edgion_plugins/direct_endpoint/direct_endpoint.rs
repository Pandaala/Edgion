use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct DirectEndpointTestSuite;

impl DirectEndpointTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DirectEndpointTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

const TEST_HOST: &str = "direct-endpoint-test.example.com";

impl DirectEndpointTestSuite {
    fn validation_success() -> TestCase {
        TestCase::new(
            "direct_endpoint_success",
            "Direct endpoint validation success",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // In direct mode/local fixtures, 127.0.0.1 is valid.
                    // In k8s mode, backend endpoints are Pod IPs and injected via env.
                    let target_ip =
                        std::env::var("EDGION_TEST_DIRECT_ENDPOINT_IP").unwrap_or_else(|_| "127.0.0.1".to_string());

                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("x-target-ip", target_ip.as_str())
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status();
                            if status.is_success() {
                                // Check if debug header is present
                                if let Some(val) = resp.headers().get("X-Direct-Endpoint") {
                                    let expected = format!("{}:30001", target_ip);
                                    if val == expected.as_str() {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Unexpected X-Direct-Endpoint header: {:?}, expected {}",
                                                val, expected
                                            ),
                                        )
                                    }
                                } else {
                                    TestResult::failed(start.elapsed(), "Missing X-Direct-Endpoint header".to_string())
                                }
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected success status, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn validation_failure() -> TestCase {
        TestCase::new(
            "direct_endpoint_failure",
            "Direct endpoint validation failure (invalid IP)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Provide an invalid endpoint (127.0.0.2) which is NOT in the EndpointSlice
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("x-target-ip", "127.0.0.2")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status();
                            // Expecting 403 Forbidden because on_invalid: Reject
                            if status == reqwest::StatusCode::FORBIDDEN {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 403 Forbidden, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn missing_header_fallback() -> TestCase {
        TestCase::new(
            "direct_endpoint_missing_fallback",
            "Missing header should fallback to normal routing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Missing x-target-ip header -> fallback -> normal routing (to 127.0.0.1 via LB)
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", TEST_HOST).send().await {
                        Ok(resp) => {
                            let status = resp.status();
                            if status.is_success() {
                                // Should NOT have X-Direct-Endpoint header because plugin yielded GoodNext
                                if resp.headers().get("X-Direct-Endpoint").is_none() {
                                    TestResult::passed(start.elapsed())
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        "Unexpected X-Direct-Endpoint header in fallback mode".to_string(),
                                    )
                                }
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected success status, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for DirectEndpointTestSuite {
    fn name(&self) -> &str {
        "DirectEndpoint"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::validation_success(),
            Self::validation_failure(),
            Self::missing_header_fallback(),
        ]
    }
}
