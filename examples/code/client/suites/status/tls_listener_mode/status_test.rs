//! TLS Listener Mode Status Test Suite
//!
//! Tests that Gateway listener status correctly reflects TLS mode support:
//! 1. Passthrough mode → Accepted=True
//! 2. Mixed mode → Accepted=False, reason=UnsupportedValue
//! 3. Terminate mode on a TLS listener → Accepted=False, Programmed=False, supportedKinds=[]

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct TlsListenerModeStatusTestSuite;

const GATEWAY_NAMESPACE: &str = "edgion-test";
const GATEWAY_NAME: &str = "tls-mode-test";

impl TlsListenerModeStatusTestSuite {
    fn find_listener_condition<'a>(gateway: &'a Value, listener_name: &str, cond_type: &str) -> Option<&'a Value> {
        gateway["status"]["listeners"].as_array()?.iter().find_map(|l| {
            if l["name"].as_str() == Some(listener_name) {
                l["conditions"]
                    .as_array()?
                    .iter()
                    .find(|c| c["type"].as_str() == Some(cond_type))
            } else {
                None
            }
        })
    }

    fn test_passthrough_accepted() -> TestCase {
        TestCase::new(
            "passthrough_accepted",
            "TLS Passthrough listener should have Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                        Ok(gateway) => {
                            let cond = Self::find_listener_condition(&gateway, "passthrough", "Accepted");
                            match cond {
                                Some(c) if c["status"].as_str() == Some("True") => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "passthrough listener: Accepted=True".to_string(),
                                ),
                                Some(c) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "passthrough listener: expected Accepted=True, got: {}",
                                        serde_json::to_string_pretty(c).unwrap_or_default()
                                    ),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "passthrough listener: Accepted condition not found. listeners: {}",
                                        serde_json::to_string_pretty(&gateway["status"]["listeners"])
                                            .unwrap_or_default()
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

    fn test_mixed_unsupported() -> TestCase {
        TestCase::new(
            "mixed_unsupported_tls_mode",
            "TLS Mixed listener should have Accepted=False with reason UnsupportedValue",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                        Ok(gateway) => {
                            let cond = Self::find_listener_condition(&gateway, "mixed", "Accepted");
                            match cond {
                                Some(c)
                                    if c["status"].as_str() == Some("False")
                                        && c["reason"].as_str() == Some("UnsupportedValue") =>
                                {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "mixed listener: Accepted=False(UnsupportedValue)".to_string(),
                                    )
                                }
                                Some(c) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "mixed listener: expected Accepted=False(UnsupportedValue), got: {}",
                                        serde_json::to_string_pretty(c).unwrap_or_default()
                                    ),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "mixed listener: Accepted condition not found. listeners: {}",
                                        serde_json::to_string_pretty(&gateway["status"]["listeners"])
                                            .unwrap_or_default()
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

    fn test_terminate_unsupported() -> TestCase {
        TestCase::new(
            "terminate_unsupported_tls_mode",
            "TLS Terminate listener should have Accepted=False(UnsupportedValue), Programmed=False, and no supportedKinds",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                        Ok(gateway) => {
                            let generation = gateway["metadata"]["generation"].as_i64();
                            let listener = gateway["status"]["listeners"]
                                .as_array()
                                .and_then(|listeners| listeners.iter().find(|l| l["name"].as_str() == Some("terminate")));

                            let Some(listener) = listener else {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "terminate listener not found. listeners: {}",
                                        serde_json::to_string_pretty(&gateway["status"]["listeners"])
                                            .unwrap_or_default()
                                    ),
                                );
                            };

                            let accepted = Self::find_listener_condition(&gateway, "terminate", "Accepted");
                            match accepted {
                                Some(c)
                                    if c["status"].as_str() == Some("False")
                                        && c["reason"].as_str() == Some("UnsupportedValue")
                                        && c["observedGeneration"].as_i64() == generation => {}
                                Some(c) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "terminate listener: expected Accepted=False(UnsupportedValue) with observedGeneration={:?}, got: {}",
                                            generation,
                                            serde_json::to_string_pretty(c).unwrap_or_default()
                                        ),
                                    );
                                }
                                None => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "terminate listener: Accepted condition not found. listener: {}",
                                            serde_json::to_string_pretty(listener).unwrap_or_default()
                                        ),
                                    );
                                }
                            }

                            let programmed = Self::find_listener_condition(&gateway, "terminate", "Programmed");
                            match programmed {
                                Some(c)
                                    if c["status"].as_str() == Some("False")
                                        && c["observedGeneration"].as_i64() == generation => {}
                                Some(c) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "terminate listener: expected Programmed=False with observedGeneration={:?}, got: {}",
                                            generation,
                                            serde_json::to_string_pretty(c).unwrap_or_default()
                                        ),
                                    );
                                }
                                None => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "terminate listener: Programmed condition not found. listener: {}",
                                            serde_json::to_string_pretty(listener).unwrap_or_default()
                                        ),
                                    );
                                }
                            }

                            let supported_kinds_empty = listener["supportedKinds"]
                                .as_array()
                                .is_some_and(|kinds| kinds.is_empty());
                            if !supported_kinds_empty {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "terminate listener: expected supportedKinds=[], got: {}",
                                        serde_json::to_string_pretty(&listener["supportedKinds"]).unwrap_or_default()
                                    ),
                                );
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                "terminate listener: Accepted=False(UnsupportedValue), Programmed=False, supportedKinds=[]"
                                    .to_string(),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TlsListenerModeStatusTestSuite {
    fn name(&self) -> &str {
        "TLS Listener Mode Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_passthrough_accepted(),
            Self::test_mixed_unsupported(),
            Self::test_terminate_unsupported(),
        ]
    }
}
