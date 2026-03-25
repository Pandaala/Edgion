// PartiallyInvalid Test suite
//
// Tests the PartiallyInvalid condition behavior:
// 1. Partial invalid: rule[0] has valid backend, rule[1] has nonexistent backend
//    - /valid should proxy successfully (200)
//    - /invalid-backend should return 404 (rule skipped by Gateway)
// 2. All invalid: both rules have nonexistent backends
//    - /bad-one and /bad-two should return 404 (route not accepted)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct PartiallyInvalidTestSuite;

impl PartiallyInvalidTestSuite {
    fn test_valid_rule_proxies() -> TestCase {
        TestCase::new(
            "valid_rule_proxies",
            "Rule[0] with valid backend should proxy successfully",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let request = ctx
                        .http_client
                        .get(format!("{}/valid", ctx.http_url()))
                        .header("Host", "partially-invalid.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Valid rule proxied successfully: 200 OK".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 for valid rule, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_invalid_rule_skipped() -> TestCase {
        TestCase::new(
            "invalid_rule_skipped",
            "Rule[1] with nonexistent backend should be skipped (404)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let request = ctx
                        .http_client
                        .get(format!("{}/invalid-backend", ctx.http_url()))
                        .header("Host", "partially-invalid.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Invalid rule skipped: 404 as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 404 for skipped invalid rule, got {} (rule was not skipped)",
                                        status
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_all_invalid_route_rejected() -> TestCase {
        TestCase::new(
            "all_invalid_route_rejected",
            "Route with ALL invalid rules should be rejected (404 for any path)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let request = ctx
                        .http_client
                        .get(format!("{}/bad-one", ctx.http_url()))
                        .header("Host", "all-invalid.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "All-invalid route rejected: 404 as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 404 for all-invalid route, got {} (route should not be accepted)",
                                        status
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for PartiallyInvalidTestSuite {
    fn name(&self) -> &str {
        "PartiallyInvalid"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/PartiallyInvalid")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_valid_rule_proxies(),
            Self::test_invalid_rule_skipped(),
            Self::test_all_invalid_route_rejected(),
        ]
    }
}
