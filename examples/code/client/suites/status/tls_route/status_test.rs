//! TLSRoute Status Test Suite
//!
//! Verifies that invalid backend refs affect ResolvedRefs without flipping Accepted.

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct TlsRouteStatusTestSuite;

const ROUTE_NAMESPACE: &str = "edgion-default";

impl TlsRouteStatusTestSuite {
    fn has_condition_with_reason(status: &Value, condition_type: &str, expected_status: &str, reason: &str) -> bool {
        if let Some(parents) = status["status"]["parents"].as_array() {
            for parent in parents {
                if let Some(conditions) = parent["conditions"].as_array() {
                    for cond in conditions {
                        if cond["type"].as_str() == Some(condition_type)
                            && cond["status"].as_str() == Some(expected_status)
                            && cond["reason"].as_str() == Some(reason)
                        {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn test_nonexistent_backend() -> TestCase {
        TestCase::new(
            "nonexistent_backend",
            "TLSRoute with non-existent Service should have Accepted=True and ResolvedRefs=False(BackendNotFound)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "TLSRoute", ROUTE_NAMESPACE, "tls-nonexistent-backend")
                        .await
                    {
                        Ok(route) => {
                            if !helpers::has_condition(&route, "Accepted", "True") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                );
                            }

                            if Self::has_condition_with_reason(&route, "ResolvedRefs", "False", "BackendNotFound") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=True, ResolvedRefs=False(BackendNotFound) as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(BackendNotFound), got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn test_invalid_backend_kind() -> TestCase {
        TestCase::new(
            "invalid_backend_kind",
            "TLSRoute with kind=Foo backend should have Accepted=True and ResolvedRefs=False(InvalidKind)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "TLSRoute", ROUTE_NAMESPACE, "tls-invalid-kind").await {
                        Ok(route) => {
                            if !helpers::has_condition(&route, "Accepted", "True") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                );
                            }

                            if Self::has_condition_with_reason(&route, "ResolvedRefs", "False", "InvalidKind") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=True, ResolvedRefs=False(InvalidKind) as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(InvalidKind), got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TlsRouteStatusTestSuite {
    fn name(&self) -> &str {
        "TLSRoute Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_nonexistent_backend(), Self::test_invalid_backend_kind()]
    }
}
