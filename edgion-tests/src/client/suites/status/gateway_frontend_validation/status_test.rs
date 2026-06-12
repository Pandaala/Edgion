//! Gateway frontend default validation status suite.
//!
//! Verifies that an invalid `spec.tls.frontend.default.validation.caCertificateRefs`
//! only impacts HTTPS listeners that inherit the default frontend validation:
//! - HTTPS listener: ResolvedRefs=False(InvalidCACertificateRef),
//!   Accepted=False(NoValidCACertificate), Programmed=False
//! - HTTP listener: remains Accepted=True, ResolvedRefs=True, Programmed=True
//! - All relevant listener conditions should carry the current observedGeneration

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct GatewayFrontendValidationStatusTestSuite;

const GATEWAY_NAMESPACE: &str = "edgion-test";
const GATEWAY_NAME: &str = "frontend-validation-invalid-default";
const HTTPS_LISTENER: &str = "https-invalid-default";
const HTTP_LISTENER: &str = "http-unaffected";

fn find_listener<'a>(gateway: &'a Value, listener_name: &str) -> Option<&'a Value> {
    gateway["status"]["listeners"]
        .as_array()?
        .iter()
        .find(|listener| listener["name"].as_str() == Some(listener_name))
}

fn find_listener_condition<'a>(gateway: &'a Value, listener_name: &str, cond_type: &str) -> Option<&'a Value> {
    find_listener(gateway, listener_name)?["conditions"]
        .as_array()?
        .iter()
        .find(|cond| cond["type"].as_str() == Some(cond_type))
}

fn assert_condition(
    gateway: &Value,
    listener_name: &str,
    cond_type: &str,
    expected_status: &str,
    expected_reason: &str,
    expected_generation: Option<i64>,
) -> Result<(), String> {
    let cond = find_listener_condition(gateway, listener_name, cond_type).ok_or_else(|| {
        format!(
            "listener '{}' missing condition '{}'. listeners: {}",
            listener_name,
            cond_type,
            serde_json::to_string_pretty(&gateway["status"]["listeners"]).unwrap_or_default()
        )
    })?;

    let actual_status = cond["status"].as_str().unwrap_or("<missing>");
    let actual_reason = cond["reason"].as_str().unwrap_or("<missing>");
    let actual_generation = cond["observedGeneration"].as_i64();

    if actual_status != expected_status {
        return Err(format!(
            "listener '{}' condition '{}' expected status {}, got {}: {}",
            listener_name,
            cond_type,
            expected_status,
            actual_status,
            serde_json::to_string_pretty(cond).unwrap_or_default()
        ));
    }

    if actual_reason != expected_reason {
        return Err(format!(
            "listener '{}' condition '{}' expected reason {}, got {}: {}",
            listener_name,
            cond_type,
            expected_reason,
            actual_reason,
            serde_json::to_string_pretty(cond).unwrap_or_default()
        ));
    }

    if actual_generation != expected_generation {
        return Err(format!(
            "listener '{}' condition '{}' expected observedGeneration {:?}, got {:?}: {}",
            listener_name,
            cond_type,
            expected_generation,
            actual_generation,
            serde_json::to_string_pretty(cond).unwrap_or_default()
        ));
    }

    Ok(())
}

impl GatewayFrontendValidationStatusTestSuite {
    fn test_invalid_default_impacts_https_listener() -> TestCase {
        TestCase::new(
            "invalid_default_impacts_https_listener",
            "Invalid default frontend validation should set HTTPS listener conditions and observedGeneration",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    let gateway =
                        match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                            Ok(gateway) => gateway,
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        };

                    let generation = gateway["metadata"]["generation"].as_i64();
                    let checks = [
                        ("ResolvedRefs", "False", "InvalidCACertificateRef"),
                        ("Accepted", "False", "NoValidCACertificate"),
                        ("Programmed", "False", "Invalid"),
                    ];

                    for (cond_type, status, reason) in checks {
                        if let Err(err) =
                            assert_condition(&gateway, HTTPS_LISTENER, cond_type, status, reason, generation)
                        {
                            return TestResult::failed(start.elapsed(), err);
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "HTTPS listener correctly reports invalid default frontend validation".to_string(),
                    )
                })
            },
        )
    }

    fn test_http_listener_unaffected() -> TestCase {
        TestCase::new(
            "http_listener_unaffected_by_invalid_default",
            "Invalid default frontend validation should not affect HTTP listener conditions",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    let gateway =
                        match helpers::fetch_resource_status(&ctx, "Gateway", GATEWAY_NAMESPACE, GATEWAY_NAME).await {
                            Ok(gateway) => gateway,
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        };

                    let generation = gateway["metadata"]["generation"].as_i64();
                    let checks = [
                        ("ResolvedRefs", "True", "ResolvedRefs"),
                        ("Accepted", "True", "Accepted"),
                        ("Programmed", "True", "Programmed"),
                    ];

                    for (cond_type, status, reason) in checks {
                        if let Err(err) =
                            assert_condition(&gateway, HTTP_LISTENER, cond_type, status, reason, generation)
                        {
                            return TestResult::failed(start.elapsed(), err);
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "HTTP listener remains healthy when invalid default frontend validation exists".to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GatewayFrontendValidationStatusTestSuite {
    fn name(&self) -> &str {
        "Gateway Frontend Validation Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_invalid_default_impacts_https_listener(),
            Self::test_http_listener_unaffected(),
        ]
    }
}
