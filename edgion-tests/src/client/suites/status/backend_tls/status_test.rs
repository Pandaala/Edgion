use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct BackendTlsStatusTestSuite;

const POLICY_NAMESPACE: &str = "edgion-test";

fn is_k8s_mode() -> bool {
    matches!(
        std::env::var("EDGION_TEST_K8S_MODE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

impl BackendTlsStatusTestSuite {
    fn test_system_ca_accepted() -> TestCase {
        TestCase::new(
            "system_ca_accepted",
            "BackendTLSPolicy with wellKnownCACertificates=System should be Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", POLICY_NAMESPACE, "system-ca").await
                    {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "Accepted", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=True as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
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

    fn test_system_ca_resolved_refs() -> TestCase {
        TestCase::new(
            "system_ca_resolved_refs",
            "BackendTLSPolicy with wellKnownCACertificates=System should have ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", POLICY_NAMESPACE, "system-ca").await
                    {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "ResolvedRefs", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=True as expected (system CA, no Secret needed)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=True, got status: {}",
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
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

    fn test_both_rejected() -> TestCase {
        TestCase::new(
            "both_rejected",
            "BackendTLSPolicy with both caCertificateRefs and wellKnownCACertificates should be Accepted=False",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if is_k8s_mode() {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            "K8s API server rejects mutually exclusive BackendTLSPolicy validation before controller status"
                                .to_string(),
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", POLICY_NAMESPACE, "both-invalid")
                        .await
                    {
                        Ok(policy) => {
                            if helpers::has_condition_with_message(&policy, "Accepted", "False", "mutually exclusive") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=False with mutually exclusive message as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=False with 'mutually exclusive', got status: {}",
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
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

    fn test_neither_rejected() -> TestCase {
        TestCase::new(
            "neither_rejected",
            "BackendTLSPolicy with neither caCertificateRefs nor wellKnownCACertificates should be Accepted=False",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if is_k8s_mode() {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            "K8s API server rejects missing BackendTLSPolicy validation source before controller status"
                                .to_string(),
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", POLICY_NAMESPACE, "neither-invalid")
                        .await
                    {
                        Ok(policy) => {
                            if helpers::has_condition_with_message(&policy, "Accepted", "False", "must specify either")
                            {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=False with 'must specify either' message as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=False with 'must specify either', got status: {}",
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
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

    fn test_configmap_ca_accepted() -> TestCase {
        TestCase::new(
            "configmap_ca_accepted",
            "BackendTLSPolicy with ConfigMap caCertificateRef should be Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", POLICY_NAMESPACE, "configmap-ca")
                        .await
                    {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "Accepted", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=True as expected for ConfigMap CA ref".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
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

    fn test_configmap_ca_resolved_refs() -> TestCase {
        TestCase::new(
            "configmap_ca_resolved_refs",
            "BackendTLSPolicy with valid ConfigMap caCertificateRef should have ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", POLICY_NAMESPACE, "configmap-ca")
                        .await
                    {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "ResolvedRefs", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=True as expected (ConfigMap CA resolved)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=True, got status: {}",
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
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

    fn test_configmap_ca_missing_unresolved() -> TestCase {
        TestCase::new(
            "configmap_ca_missing_unresolved",
            "BackendTLSPolicy referencing non-existent ConfigMap should have ResolvedRefs=False",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(
                        &ctx,
                        "BackendTLSPolicy",
                        POLICY_NAMESPACE,
                        "configmap-ca-missing",
                    )
                    .await
                    {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "ResolvedRefs", "False") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False as expected (ConfigMap not found)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False, got status: {}",
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
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
impl TestSuite for BackendTlsStatusTestSuite {
    fn name(&self) -> &str {
        "BackendTLSPolicy Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_system_ca_accepted(),
            Self::test_system_ca_resolved_refs(),
            Self::test_both_rejected(),
            Self::test_neither_rejected(),
            Self::test_configmap_ca_accepted(),
            Self::test_configmap_ca_resolved_refs(),
            Self::test_configmap_ca_missing_unresolved(),
        ]
    }
}
