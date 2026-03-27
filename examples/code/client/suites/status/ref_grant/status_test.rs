//! ReferenceGrant Status Test Suite
//!
//! Tests the status system for cross-namespace references:
//! 1. Cross-ns reference with ReferenceGrant → ResolvedRefs=True
//! 2. Cross-ns reference without ReferenceGrant → ResolvedRefs=False
//! 3. ReferenceGrant arriving later triggers requeue and status update
//! 4. Multiple parent_refs have independent status

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct RefGrantStatusTestSuite;

const ROUTE_NAMESPACE: &str = "edgion-default";

impl RefGrantStatusTestSuite {
    /// Check if status has ResolvedRefs=False condition with specific reason
    fn has_resolved_refs_false_with_reason(status: &Value, reason: &str) -> bool {
        if let Some(parents) = status["status"]["parents"].as_array() {
            for parent in parents {
                if let Some(conditions) = parent["conditions"].as_array() {
                    for cond in conditions {
                        if cond["type"].as_str() == Some("ResolvedRefs")
                            && cond["status"].as_str() == Some("False")
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

    /// Test: Cross-namespace reference with ReferenceGrant should have ResolvedRefs=True
    fn test_cross_ns_allowed() -> TestCase {
        TestCase::new(
            "cross_ns_allowed",
            "Cross-ns reference with ReferenceGrant should have ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait a bit for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "cross-ns-route").await {
                        Ok(route) => {
                            if helpers::has_condition(&route, "ResolvedRefs", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=True as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=True, got status: {}",
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

    /// Test: Cross-namespace reference without ReferenceGrant should have ResolvedRefs=False
    fn test_cross_ns_denied() -> TestCase {
        TestCase::new(
            "cross_ns_denied",
            "Cross-ns reference without ReferenceGrant should have ResolvedRefs=False",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait a bit for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "cross-ns-denied").await {
                        Ok(route) => {
                            if Self::has_resolved_refs_false_with_reason(&route, "RefNotPermitted") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(RefNotPermitted) as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(RefNotPermitted), got status: {}",
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

    /// Test: Route with multiple parent_refs should have independent status for each
    fn test_multi_parent() -> TestCase {
        TestCase::new(
            "multi_parent",
            "Route with multiple parent_refs should have status for each parent",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait a bit for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "multi-parent").await {
                        Ok(route) => {
                            if let Some(parents) = route["status"]["parents"].as_array() {
                                if parents.len() >= 2 {
                                    // Check that each parent has conditions
                                    let all_have_conditions = parents
                                        .iter()
                                        .all(|p| p["conditions"].as_array().map(|c| !c.is_empty()).unwrap_or(false));

                                    if all_have_conditions {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Found {} parents with conditions", parents.len()),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            "Not all parents have conditions".to_string(),
                                        )
                                    }
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!("Expected >= 2 parents, got {}", parents.len()),
                                    )
                                }
                            } else {
                                TestResult::failed(start.elapsed(), "No parents in status".to_string())
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test: Status has Accepted condition
    fn test_has_accepted_condition() -> TestCase {
        TestCase::new(
            "has_accepted_condition",
            "Route status should have Accepted condition",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait a bit for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "cross-ns-route").await {
                        Ok(route) => {
                            let has_accepted = if let Some(parents) = route["status"]["parents"].as_array() {
                                parents.iter().any(|p| {
                                    p["conditions"].as_array().is_some_and(|conds| {
                                        conds.iter().any(|c| c["type"].as_str() == Some("Accepted"))
                                    })
                                })
                            } else {
                                false
                            };

                            if has_accepted {
                                TestResult::passed_with_message(start.elapsed(), "Found Accepted condition".to_string())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted condition, got status: {}",
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

    /// Test: GatewayClass has Accepted=True condition
    fn test_gateway_class_accepted() -> TestCase {
        TestCase::new(
            "gateway_class_accepted",
            "GatewayClass status should have Accepted=True condition",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_cluster_resource_status(&ctx, "GatewayClass", "public-gateway").await {
                        Ok(gc) => {
                            let accepted_true = gc["status"]["conditions"].as_array().is_some_and(|conds| {
                                conds.iter().any(|c| {
                                    c["type"].as_str() == Some("Accepted") && c["status"].as_str() == Some("True")
                                })
                            });

                            if accepted_true {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "GatewayClass has Accepted=True".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected GatewayClass Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&gc["status"]).unwrap_or_default()
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

    /// Test: HTTPRoute with invalid backend kind should have ResolvedRefs=False/InvalidKind
    fn test_invalid_backend_kind() -> TestCase {
        TestCase::new(
            "invalid_backend_kind",
            "HTTPRoute with kind=Foo backend should have ResolvedRefs=False(InvalidKind)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "invalid-kind-route").await
                    {
                        Ok(route) => {
                            if Self::has_resolved_refs_false_with_reason(&route, "InvalidKind") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(InvalidKind) as expected".to_string(),
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

    /// Test: HTTPRoute with non-existent backend should have ResolvedRefs=False/BackendNotFound
    fn test_nonexistent_backend() -> TestCase {
        TestCase::new(
            "nonexistent_backend",
            "HTTPRoute with non-existent Service should have ResolvedRefs=False(BackendNotFound)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "nonexistent-backend")
                        .await
                    {
                        Ok(route) => {
                            if Self::has_resolved_refs_false_with_reason(&route, "BackendNotFound") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(BackendNotFound) as expected".to_string(),
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

    /// Test: HTTPRoute with some invalid rules should have PartiallyInvalid=True
    fn test_partially_invalid() -> TestCase {
        TestCase::new(
            "partially_invalid",
            "HTTPRoute with one valid and one cross-ns-denied rule should have PartiallyInvalid=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "refgrant-partially-invalid").await
                    {
                        Ok(route) => {
                            let has_partially_invalid = if let Some(parents) = route["status"]["parents"].as_array() {
                                parents.iter().any(|p| {
                                    p["conditions"].as_array().is_some_and(|conds| {
                                        conds.iter().any(|c| {
                                            c["type"].as_str() == Some("PartiallyInvalid")
                                                && c["status"].as_str() == Some("True")
                                        })
                                    })
                                })
                            } else {
                                false
                            };

                            if has_partially_invalid {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "PartiallyInvalid=True as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected PartiallyInvalid=True, got status: {}",
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
impl TestSuite for RefGrantStatusTestSuite {
    fn name(&self) -> &str {
        "ReferenceGrant Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_gateway_class_accepted(),
            Self::test_has_accepted_condition(),
            Self::test_cross_ns_allowed(),
            Self::test_cross_ns_denied(),
            Self::test_multi_parent(),
            Self::test_invalid_backend_kind(),
            Self::test_nonexistent_backend(),
            Self::test_partially_invalid(),
        ]
    }
}
