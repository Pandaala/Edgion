//! Tests that `observedGeneration` in status conditions matches `metadata.generation`.
//!
//! Covers:
//! - Gateway top-level conditions (Accepted, Programmed)
//! - Gateway per-listener conditions (Accepted, ResolvedRefs)
//! - GatewayClass conditions (Accepted, SupportedVersion)

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct ObservedGenerationStatusTestSuite;

const GATEWAY_NAMESPACE: &str = "edgion-test";
const GATEWAY_NAME: &str = "observed-gen-gateway";
const GATEWAY_CLASS_NAME: &str = "observed-gen-class";

/// Extract all `observedGeneration` values from a conditions array.
fn extract_observed_generations(conditions: &[Value]) -> Vec<(String, Option<i64>)> {
    conditions
        .iter()
        .filter_map(|c| {
            let type_ = c["type"].as_str()?.to_string();
            let og = c["observedGeneration"].as_i64();
            Some((type_, og))
        })
        .collect()
}

impl ObservedGenerationStatusTestSuite {
    /// Gateway conditions should all have observedGeneration == metadata.generation
    fn test_gateway_conditions_observed_generation() -> TestCase {
        TestCase::new(
            "gateway_conditions_observed_generation",
            "Gateway status.conditions[*].observedGeneration should match metadata.generation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    let gateway =
                        match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                            Ok(gw) => gw,
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        };

                    let metadata_gen = gateway["metadata"]["generation"].as_i64();
                    let conditions = match gateway["status"]["conditions"].as_array() {
                        Some(c) => c,
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "No status.conditions found. status: {}",
                                    serde_json::to_string_pretty(&gateway["status"]).unwrap_or_default()
                                ),
                            );
                        }
                    };

                    if conditions.is_empty() {
                        return TestResult::failed(start.elapsed(), "status.conditions is empty".into());
                    }

                    let observations = extract_observed_generations(conditions);
                    let mut failures = Vec::new();

                    for (type_, og) in &observations {
                        if *og != metadata_gen {
                            failures.push(format!(
                                "condition '{}': observedGeneration={:?}, expected {:?}",
                                type_, og, metadata_gen
                            ));
                        }
                    }

                    if failures.is_empty() {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "All {} conditions have observedGeneration={:?}",
                                observations.len(),
                                metadata_gen
                            ),
                        )
                    } else {
                        TestResult::failed(start.elapsed(), failures.join("; "))
                    }
                })
            },
        )
    }

    /// Gateway listener conditions should all have observedGeneration == metadata.generation
    fn test_gateway_listener_conditions_observed_generation() -> TestCase {
        TestCase::new(
            "gateway_listener_conditions_observed_generation",
            "Gateway status.listeners[*].conditions[*].observedGeneration should match metadata.generation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    let gateway =
                        match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                            Ok(gw) => gw,
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        };

                    let metadata_gen = gateway["metadata"]["generation"].as_i64();
                    let listeners = match gateway["status"]["listeners"].as_array() {
                        Some(l) => l,
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "No status.listeners found. status: {}",
                                    serde_json::to_string_pretty(&gateway["status"]).unwrap_or_default()
                                ),
                            );
                        }
                    };

                    let mut failures = Vec::new();
                    for listener in listeners {
                        let listener_name = listener["name"].as_str().unwrap_or("<unknown>");
                        if let Some(conditions) = listener["conditions"].as_array() {
                            for (type_, og) in extract_observed_generations(conditions) {
                                if og != metadata_gen {
                                    failures.push(format!(
                                        "listener '{}' condition '{}': observedGeneration={:?}, expected {:?}",
                                        listener_name, type_, og, metadata_gen
                                    ));
                                }
                            }
                        }
                    }

                    if failures.is_empty() {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("All listener conditions have observedGeneration={:?}", metadata_gen),
                        )
                    } else {
                        TestResult::failed(start.elapsed(), failures.join("; "))
                    }
                })
            },
        )
    }

    /// GatewayClass conditions should have observedGeneration matching its metadata.generation.
    fn test_gateway_class_conditions_observed_generation() -> TestCase {
        TestCase::new(
            "gateway_class_conditions_observed_generation",
            "GatewayClass status.conditions[*].observedGeneration should match metadata.generation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    let gc =
                        match helpers::fetch_cluster_resource_status(&ctx, "GatewayClass", GATEWAY_CLASS_NAME).await {
                            Ok(gc) => gc,
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        };

                    let metadata_gen = gc["metadata"]["generation"].as_i64();
                    let conditions = match gc["status"]["conditions"].as_array() {
                        Some(c) => c,
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "No status.conditions found. status: {}",
                                    serde_json::to_string_pretty(&gc["status"]).unwrap_or_default()
                                ),
                            );
                        }
                    };

                    if conditions.is_empty() {
                        return TestResult::failed(start.elapsed(), "status.conditions is empty".into());
                    }

                    let observations = extract_observed_generations(conditions);
                    let mut failures = Vec::new();

                    for (type_, og) in &observations {
                        if *og != metadata_gen {
                            failures.push(format!(
                                "condition '{}': observedGeneration={:?}, expected {:?}",
                                type_, og, metadata_gen
                            ));
                        }
                    }

                    if failures.is_empty() {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "All {} GatewayClass conditions have observedGeneration={:?}",
                                observations.len(),
                                metadata_gen
                            ),
                        )
                    } else {
                        TestResult::failed(start.elapsed(), failures.join("; "))
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for ObservedGenerationStatusTestSuite {
    fn name(&self) -> &str {
        "ObservedGeneration Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_gateway_conditions_observed_generation(),
            Self::test_gateway_listener_conditions_observed_generation(),
            Self::test_gateway_class_conditions_observed_generation(),
        ]
    }
}
