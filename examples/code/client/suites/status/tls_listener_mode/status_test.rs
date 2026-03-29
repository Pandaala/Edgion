//! TLS Listener Mode Status Test Suite
//!
//! Tests that Gateway listener status correctly reflects TLS mode support:
//! 1. Passthrough mode → Accepted=True
//! 2. Mixed mode → Accepted=False, reason=UnsupportedTLSMode

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
            "TLS Mixed listener should have Accepted=False with reason UnsupportedTLSMode",
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
                                        && c["reason"].as_str() == Some("UnsupportedTLSMode") =>
                                {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "mixed listener: Accepted=False(UnsupportedTLSMode)".to_string(),
                                    )
                                }
                                Some(c) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "mixed listener: expected Accepted=False(UnsupportedTLSMode), got: {}",
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
}

#[async_trait]
impl TestSuite for TlsListenerModeStatusTestSuite {
    fn name(&self) -> &str {
        "TLS Listener Mode Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_passthrough_accepted(), Self::test_mixed_unsupported()]
    }
}
