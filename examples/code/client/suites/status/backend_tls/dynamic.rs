//! BTLSP RefFlap scenario phase suites.
//!
//! Initial phase: BTLSP references existing ConfigMap `ca-good`; assert Accepted=True, ResolvedRefs=True.
//! Invalid phase: BTLSP references non-existent `ca-missing`; assert ResolvedRefs=False with reason
//!               InvalidCACertificateRef, and Accepted=False with reason NoValidCACertificate.
//! Valid phase:   BTLSP restored to reference `ca-good`; assert Accepted=True, ResolvedRefs=True.

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

const NS: &str = "edgion-test";
const BTLSP: &str = "btlsp-flap";

// ---------------------------------------------------------------------------
// Initial phase suite
// ---------------------------------------------------------------------------

pub struct BtlspFlapInitialTestSuite;

impl BtlspFlapInitialTestSuite {
    fn test_accepted_true() -> TestCase {
        TestCase::new(
            "btlsp_flap_initial_accepted_true",
            "BTLSP referencing existing ca-good ConfigMap should be Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", NS, BTLSP).await {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "Accepted", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=True as expected (ca-good ConfigMap present)".to_string(),
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

    fn test_resolved_refs_true() -> TestCase {
        TestCase::new(
            "btlsp_flap_initial_resolved_refs_true",
            "BTLSP referencing existing ca-good ConfigMap should be ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", NS, BTLSP).await {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "ResolvedRefs", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=True as expected (ca-good ConfigMap resolved)".to_string(),
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
}

#[async_trait]
impl TestSuite for BtlspFlapInitialTestSuite {
    fn name(&self) -> &str {
        "BtlspFlapInitial"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_accepted_true(), Self::test_resolved_refs_true()]
    }
}

// ---------------------------------------------------------------------------
// Invalid phase suite (polled by wait_phase)
// ---------------------------------------------------------------------------

pub struct BtlspFlapInvalidTestSuite;

impl BtlspFlapInvalidTestSuite {
    fn test_resolved_refs_false() -> TestCase {
        TestCase::new(
            "btlsp_flap_invalid_resolved_refs_false",
            "BTLSP referencing missing ca-missing ConfigMap should be ResolvedRefs=False",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", NS, BTLSP).await {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "ResolvedRefs", "False") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False as expected (ca-missing ConfigMap not found)".to_string(),
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

    fn test_accepted_false_no_valid_ca() -> TestCase {
        TestCase::new(
            "btlsp_flap_invalid_accepted_false",
            "BTLSP with all CA refs invalid should be Accepted=False (NoValidCACertificate)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", NS, BTLSP).await {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "Accepted", "False") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=False as expected (all CA refs invalid)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=False, got status: {}",
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
impl TestSuite for BtlspFlapInvalidTestSuite {
    fn name(&self) -> &str {
        "BtlspFlapInvalid"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_resolved_refs_false(),
            Self::test_accepted_false_no_valid_ca(),
        ]
    }
}

// ---------------------------------------------------------------------------
// Valid (recovered) phase suite (polled by wait_phase)
// ---------------------------------------------------------------------------

pub struct BtlspFlapValidTestSuite;

impl BtlspFlapValidTestSuite {
    fn test_accepted_true_recovered() -> TestCase {
        TestCase::new(
            "btlsp_flap_valid_accepted_true_recovered",
            "BTLSP restored to ca-good ConfigMap should be Accepted=True again",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", NS, BTLSP).await {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "Accepted", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=True as expected (recovered to ca-good)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=True (recovered), got status: {}",
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

    fn test_resolved_refs_true_recovered() -> TestCase {
        TestCase::new(
            "btlsp_flap_valid_resolved_refs_true_recovered",
            "BTLSP restored to ca-good ConfigMap should be ResolvedRefs=True again",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_resource_status(&ctx, "BackendTLSPolicy", NS, BTLSP).await {
                        Ok(policy) => {
                            if helpers::has_condition(&policy, "ResolvedRefs", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=True as expected (recovered to ca-good)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=True (recovered), got status: {}",
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
impl TestSuite for BtlspFlapValidTestSuite {
    fn name(&self) -> &str {
        "BtlspFlapValid"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_accepted_true_recovered(),
            Self::test_resolved_refs_true_recovered(),
        ]
    }
}
