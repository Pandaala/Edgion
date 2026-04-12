//! TLS Listener Mode Status Test Suite
//!
//! Tests that Gateway listener status correctly reflects TLS mode support:
//! 1. TLS/Passthrough → Accepted=True
//! 2. HTTPS/Passthrough → Accepted=False, reason=UnsupportedValue (HTTPS only supports Terminate)
//! 3. TLS/Terminate → Accepted=True
//! 4. Two TLS listeners on the same port with different modes → Accepted=False, reason=ProtocolConflict

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

    fn test_https_passthrough_unsupported() -> TestCase {
        TestCase::new(
            "https_passthrough_unsupported",
            "HTTPS listener with Passthrough mode should have Accepted=False with reason UnsupportedValue",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                        Ok(gateway) => {
                            let cond = Self::find_listener_condition(&gateway, "unsupported-mode", "Accepted");
                            match cond {
                                Some(c)
                                    if c["status"].as_str() == Some("False")
                                        && c["reason"].as_str() == Some("UnsupportedValue") =>
                                {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "unsupported-mode listener: Accepted=False(UnsupportedValue)".to_string(),
                                    )
                                }
                                Some(c) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "unsupported-mode listener: expected Accepted=False(UnsupportedValue), got: {}",
                                        serde_json::to_string_pretty(c).unwrap_or_default()
                                    ),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "unsupported-mode listener: Accepted condition not found. listeners: {}",
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

    fn test_mixed_tls_conflict() -> TestCase {
        TestCase::new(
            "mixed_tls_conflict",
            "Two TLS listeners on the same port with different modes should both have Accepted=False, reason=ProtocolConflict",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                        Ok(gateway) => {
                            let check = |listener_name: &str| -> Result<(), String> {
                                let cond = Self::find_listener_condition(&gateway, listener_name, "Accepted");
                                match cond {
                                    Some(c)
                                        if c["status"].as_str() == Some("False")
                                            && c["reason"].as_str() == Some("ProtocolConflict") =>
                                    {
                                        Ok(())
                                    }
                                    Some(c) => Err(format!(
                                        "{}: expected Accepted=False(ProtocolConflict), got: {}",
                                        listener_name,
                                        serde_json::to_string_pretty(c).unwrap_or_default()
                                    )),
                                    None => Err(format!(
                                        "{}: Accepted condition not found. listeners: {}",
                                        listener_name,
                                        serde_json::to_string_pretty(&gateway["status"]["listeners"])
                                            .unwrap_or_default()
                                    )),
                                }
                            };

                            match (check("mixed-passthrough"), check("mixed-terminate")) {
                                (Ok(()), Ok(())) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "mixed-passthrough and mixed-terminate: both Accepted=False(ProtocolConflict)"
                                        .to_string(),
                                ),
                                (Err(e), _) | (_, Err(e)) => TestResult::failed(start.elapsed(), e),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn test_terminate_accepted() -> TestCase {
        TestCase::new(
            "terminate_accepted",
            "TLS Terminate listener should have Accepted=True (defaults to TCP backend)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                        Ok(gateway) => {
                            let cond = Self::find_listener_condition(&gateway, "terminate", "Accepted");
                            match cond {
                                Some(c) if c["status"].as_str() == Some("True") => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "terminate listener: Accepted=True".to_string(),
                                ),
                                Some(c) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "terminate listener: expected Accepted=True, got: {}",
                                        serde_json::to_string_pretty(c).unwrap_or_default()
                                    ),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "terminate listener: Accepted condition not found. listeners: {}",
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
        vec![
            Self::test_passthrough_accepted(),
            Self::test_https_passthrough_unsupported(),
            Self::test_terminate_accepted(),
            Self::test_mixed_tls_conflict(),
        ]
    }
}
