//! GRPCRoute Status Test Suite
//!
//! Verifies that the controller correctly sets status.parents conditions on GRPCRoute resources.

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct GrpcRouteStatusTestSuite;

const ROUTE_NAMESPACE: &str = "edgion-default";

impl GrpcRouteStatusTestSuite {
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

    /// Test: Basic GRPCRoute with valid backend should have Accepted=True and ResolvedRefs=True
    fn test_basic_route_accepted() -> TestCase {
        TestCase::new(
            "basic_route_accepted",
            "Basic GRPCRoute with valid backend should have Accepted=True and ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "GRPCRoute", ROUTE_NAMESPACE, "grpc-basic-route").await {
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
                            if helpers::has_condition(&route, "ResolvedRefs", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=True, ResolvedRefs=True as expected".to_string(),
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

    /// Test: GRPCRoute with non-existent backend should have ResolvedRefs=False(BackendNotFound)
    fn test_nonexistent_backend() -> TestCase {
        TestCase::new(
            "nonexistent_backend",
            "GRPCRoute with non-existent Service should have ResolvedRefs=False(BackendNotFound)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "GRPCRoute", ROUTE_NAMESPACE, "grpc-nonexistent-backend")
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

    /// Test: GRPCRoute with invalid backend kind should have ResolvedRefs=False(InvalidKind)
    fn test_invalid_backend_kind() -> TestCase {
        TestCase::new(
            "invalid_backend_kind",
            "GRPCRoute with kind=Foo backend should have ResolvedRefs=False(InvalidKind)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "GRPCRoute", ROUTE_NAMESPACE, "grpc-invalid-kind").await
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

    /// Test: GRPCRoute status.parents should be populated (non-empty)
    fn test_parents_populated() -> TestCase {
        TestCase::new(
            "parents_populated",
            "GRPCRoute status.parents should be non-empty after controller processes the route",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "GRPCRoute", ROUTE_NAMESPACE, "grpc-basic-route").await {
                        Ok(route) => {
                            let parents = route["status"]["parents"].as_array();
                            match parents {
                                Some(p) if !p.is_empty() => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("status.parents has {} entries", p.len()),
                                ),
                                Some(_) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status.parents is empty; status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status.parents is missing; status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                ),
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
impl TestSuite for GrpcRouteStatusTestSuite {
    fn name(&self) -> &str {
        "GRPCRoute Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_parents_populated(),
            Self::test_basic_route_accepted(),
            Self::test_nonexistent_backend(),
            Self::test_invalid_backend_kind(),
        ]
    }
}
