//! TLSRouteCrossNsSvc — TLSRoute cross-namespace Service ResolvedRefs status
//!
//! Two TLSRoutes exist simultaneously:
//! - tls-cross-ns-allowed (ns=app): has ReferenceGrant → ResolvedRefs=True
//! - tls-cross-ns-denied  (ns=app2): no ReferenceGrant  → ResolvedRefs=False(RefNotPermitted)

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct TlsRouteCrossNsSvcTestSuite;

impl TlsRouteCrossNsSvcTestSuite {
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

    /// TLSRoute in ns=app with ReferenceGrant should have ResolvedRefs=True
    fn test_allowed_route_resolved_true() -> TestCase {
        TestCase::new(
            "tls_cross_ns_allowed",
            "TLSRoute cross-ns with ReferenceGrant should have ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "TLSRoute", "app", "tls-cross-ns-allowed").await {
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

    /// TLSRoute in ns=app2 without ReferenceGrant should have ResolvedRefs=False(RefNotPermitted)
    fn test_denied_route_resolved_false() -> TestCase {
        TestCase::new(
            "tls_cross_ns_denied",
            "TLSRoute cross-ns without ReferenceGrant should have ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "TLSRoute", "app2", "tls-cross-ns-denied").await {
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
}

#[async_trait]
impl TestSuite for TlsRouteCrossNsSvcTestSuite {
    fn name(&self) -> &str {
        "TLSRouteCrossNsSvc"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_allowed_route_resolved_true(),
            Self::test_denied_route_resolved_false(),
        ]
    }
}
