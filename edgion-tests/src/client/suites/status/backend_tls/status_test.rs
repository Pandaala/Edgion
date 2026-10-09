use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct BackendTlsStatusTestSuite;

const POLICY_NAMESPACE: &str = "edgion-test";
/// Controller status is published asynchronously, and an unresolved policy is
/// reprocessed once its dependency store settles, so every assertion here polls
/// instead of sampling once after a fixed delay.
const STATUS_TIMEOUT: Duration = Duration::from_secs(15);

impl BackendTlsStatusTestSuite {
    fn test_system_ca_accepted() -> TestCase {
        TestCase::new(
            "system_ca_accepted",
            "BackendTLSPolicy with wellKnownCACertificates=System should be Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::wait_resource_status(
                        &ctx,
                        "BackendTLSPolicy",
                        POLICY_NAMESPACE,
                        "system-ca",
                        STATUS_TIMEOUT,
                        |policy| helpers::has_condition(policy, "Accepted", "True"),
                    )
                    .await
                    {
                        Ok(_) => {
                            TestResult::passed_with_message(start.elapsed(), "Accepted=True as expected".to_string())
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Expected Accepted=True, {error}")),
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
                    match helpers::wait_resource_status(
                        &ctx,
                        "BackendTLSPolicy",
                        POLICY_NAMESPACE,
                        "system-ca",
                        STATUS_TIMEOUT,
                        |policy| helpers::has_condition(policy, "ResolvedRefs", "True"),
                    )
                    .await
                    {
                        Ok(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "ResolvedRefs=True as expected (system CA, no Secret needed)".to_string(),
                        ),
                        Err(error) => {
                            TestResult::failed(start.elapsed(), format!("Expected ResolvedRefs=True, {error}"))
                        }
                    }
                })
            },
        )
    }

    fn test_both_rejected() -> TestCase {
        TestCase::new(
            "both_rejected",
            "Invalid BackendTLSPolicy CA selection is rejected without persistence",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resource = serde_yaml::from_str::<serde_json::Value>(include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/integration/fixtures/status-admission/BackendTLSPolicy_edgion-test_both-invalid.yaml"
                    )))
                    .expect("valid YAML fixture");
                    match helpers::expect_admin_admission_rejected(&ctx, &resource).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Admin rejected invalid CA selection with 400; resource remains absent".into(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn test_neither_rejected() -> TestCase {
        TestCase::new(
            "neither_rejected",
            "Invalid BackendTLSPolicy CA selection is rejected without persistence",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resource = serde_yaml::from_str::<serde_json::Value>(include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/integration/fixtures/status-admission/BackendTLSPolicy_edgion-test_neither-invalid.yaml"
                    )))
                    .expect("valid YAML fixture");
                    match helpers::expect_admin_admission_rejected(&ctx, &resource).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Admin rejected invalid CA selection with 400; resource remains absent".into(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
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
                    match helpers::wait_resource_status(
                        &ctx,
                        "BackendTLSPolicy",
                        POLICY_NAMESPACE,
                        "configmap-ca",
                        STATUS_TIMEOUT,
                        |policy| helpers::has_condition(policy, "Accepted", "True"),
                    )
                    .await
                    {
                        Ok(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Accepted=True as expected for ConfigMap CA ref".to_string(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("Expected Accepted=True, {error}")),
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
                    match helpers::wait_resource_status(
                        &ctx,
                        "BackendTLSPolicy",
                        POLICY_NAMESPACE,
                        "configmap-ca",
                        STATUS_TIMEOUT,
                        |policy| helpers::has_condition(policy, "ResolvedRefs", "True"),
                    )
                    .await
                    {
                        Ok(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "ResolvedRefs=True as expected (ConfigMap CA resolved)".to_string(),
                        ),
                        Err(error) => {
                            TestResult::failed(start.elapsed(), format!("Expected ResolvedRefs=True, {error}"))
                        }
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
                    match helpers::wait_resource_status(
                        &ctx,
                        "BackendTLSPolicy",
                        POLICY_NAMESPACE,
                        "configmap-ca-missing",
                        STATUS_TIMEOUT,
                        |policy| helpers::has_condition(policy, "ResolvedRefs", "False"),
                    )
                    .await
                    {
                        Ok(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "ResolvedRefs=False as expected (ConfigMap not found)".to_string(),
                        ),
                        Err(error) => {
                            TestResult::failed(start.elapsed(), format!("Expected ResolvedRefs=False, {error}"))
                        }
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
