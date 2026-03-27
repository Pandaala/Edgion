//! Hostname Intersection attachedRoutes Status Test Suite
//!
//! Tests:
//! 1. Diagnostic: print full Gateway listeners status
//! 2. Assert attachedRoutes count per listener (listener-1=2, listener-2=1, listener-3=1)
//! 3. Assert no-intersecting-hosts route has Accepted=False

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct HostnameIntersectionStatusTestSuite;

const GATEWAY_NAMESPACE: &str = "edgion-test";
const GATEWAY_NAME: &str = "hostname-intersection";

impl HostnameIntersectionStatusTestSuite {
    /// Diagnostic: fetch and print full Gateway listeners status (always passes)
    fn test_print_gateway_listeners_status() -> TestCase {
        TestCase::new(
            "print_gateway_listeners_status",
            "Diagnostic: print full Gateway listeners status for inspection",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                        Ok(gateway) => {
                            let listeners = &gateway["status"]["listeners"];
                            let pretty = serde_json::to_string_pretty(listeners).unwrap_or_else(|_| "null".to_string());
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Gateway listeners status:\n{}", pretty),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Failed to fetch Gateway: {}", e)),
                    }
                })
            },
        )
    }

    /// Assert attachedRoutes count per listener
    fn test_attached_routes_count() -> TestCase {
        TestCase::new(
            "attached_routes_count",
            "attachedRoutes: listener-1=2, listener-2=1, listener-3=1",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                        Ok(gateway) => {
                            let listeners = match gateway["status"]["listeners"].as_array() {
                                Some(l) => l,
                                None => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "No listeners in Gateway status. Full status: {}",
                                            serde_json::to_string_pretty(&gateway["status"]).unwrap_or_default()
                                        ),
                                    );
                                }
                            };

                            let expected: &[(&str, i64)] = &[("listener-1", 2), ("listener-2", 1), ("listener-3", 1)];
                            let mut errors = Vec::new();

                            for (name, expected_count) in expected {
                                let actual = listeners
                                    .iter()
                                    .find(|l| l["name"].as_str() == Some(name))
                                    .and_then(|l| l["attachedRoutes"].as_i64());

                                match actual {
                                    Some(count) if count == *expected_count => {}
                                    Some(count) => {
                                        errors.push(format!(
                                            "{}: expected attachedRoutes={}, got {}",
                                            name, expected_count, count
                                        ));
                                    }
                                    None => {
                                        errors.push(format!(
                                            "{}: attachedRoutes field missing or listener not found",
                                            name
                                        ));
                                    }
                                }
                            }

                            if errors.is_empty() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "All listener attachedRoutes counts match expected values".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), errors.join("; "))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Assert no-intersecting-hosts route has Accepted=False
    fn test_no_intersection_route_not_accepted() -> TestCase {
        TestCase::new(
            "no_intersection_route_not_accepted",
            "Route with no hostname intersection should have Accepted=False",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", GATEWAY_NAMESPACE, "no-intersecting-hosts")
                        .await
                    {
                        Ok(route) => {
                            // Print route status for diagnosis
                            let status_str = serde_json::to_string_pretty(&route["status"]).unwrap_or_default();

                            let has_accepted_false = if let Some(parents) = route["status"]["parents"].as_array() {
                                parents.iter().any(|p| {
                                    p["conditions"].as_array().is_some_and(|conds| {
                                        conds.iter().any(|c| {
                                            c["type"].as_str() == Some("Accepted")
                                                && c["status"].as_str() == Some("False")
                                        })
                                    })
                                })
                            } else {
                                false
                            };

                            if has_accepted_false {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Accepted=False as expected. Status:\n{}", status_str),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected Accepted=False, got status:\n{}", status_str),
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
impl TestSuite for HostnameIntersectionStatusTestSuite {
    fn name(&self) -> &str {
        "Hostname Intersection attachedRoutes"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_print_gateway_listeners_status(),
            Self::test_attached_routes_count(),
            Self::test_no_intersection_route_not_accepted(),
        ]
    }
}
