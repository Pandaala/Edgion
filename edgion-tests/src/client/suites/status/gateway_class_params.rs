//! GatewayClass parametersRef RefFlap scenario phase suites (controller-07).
//!
//! Missing phase: GatewayClass `params-flap-class` references EdgionGatewayConfig
//!                `params-flap-egc`, which does NOT exist → ResolvedRefs=False, Accepted=False.
//! Present phase: the EdgionGatewayConfig is created → the GatewayClass is requeued (EGC→GatewayClass
//!                requeue) and flips to ResolvedRefs=True, Accepted=True.
//! Unresolved phase: the EdgionGatewayConfig declares a missing global plugin → the owning
//!                GatewayClass reports ResolvedRefs=False while staying Accepted=True.
//! The scenario then deletes the EdgionGatewayConfig again and re-asserts the Missing phase,
//! exercising the on_delete requeue path.

use super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

const GC: &str = "params-flap-class";

// ---------------------------------------------------------------------------
// Missing phase suite (target EdgionGatewayConfig absent)
// ---------------------------------------------------------------------------

pub struct GcParamsFlapMissingTestSuite;

impl GcParamsFlapMissingTestSuite {
    fn test_resolved_refs_false() -> TestCase {
        TestCase::new(
            "gc_params_flap_missing_resolved_refs_false",
            "GatewayClass referencing a missing EdgionGatewayConfig should be ResolvedRefs=False",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_cluster_resource_status(&ctx, "GatewayClass", GC).await {
                        Ok(gc) => {
                            if helpers::has_condition(&gc, "ResolvedRefs", "False") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False as expected (EdgionGatewayConfig absent)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False, got status: {}",
                                        serde_json::to_string_pretty(&gc["status"]).unwrap_or_default()
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

    fn test_accepted_false() -> TestCase {
        TestCase::new(
            "gc_params_flap_missing_accepted_false",
            "GatewayClass referencing a missing EdgionGatewayConfig should be Accepted=False",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_cluster_resource_status(&ctx, "GatewayClass", GC).await {
                        Ok(gc) => {
                            if helpers::has_condition(&gc, "Accepted", "False") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=False as expected (parametersRef unresolved)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=False, got status: {}",
                                        serde_json::to_string_pretty(&gc["status"]).unwrap_or_default()
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
impl TestSuite for GcParamsFlapMissingTestSuite {
    fn name(&self) -> &str {
        "GcParamsFlapMissing"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_resolved_refs_false(), Self::test_accepted_false()]
    }
}

// ---------------------------------------------------------------------------
// Present phase suite (target EdgionGatewayConfig created → requeue flips to True)
// ---------------------------------------------------------------------------

pub struct GcParamsFlapPresentTestSuite;

impl GcParamsFlapPresentTestSuite {
    fn test_resolved_refs_true() -> TestCase {
        TestCase::new(
            "gc_params_flap_present_resolved_refs_true",
            "After the EdgionGatewayConfig is created, GatewayClass should become ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_cluster_resource_status(&ctx, "GatewayClass", GC).await {
                        Ok(gc) => {
                            if helpers::has_condition(&gc, "ResolvedRefs", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=True as expected (EdgionGatewayConfig resolved via requeue)"
                                        .to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=True, got status: {}",
                                        serde_json::to_string_pretty(&gc["status"]).unwrap_or_default()
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

    fn test_accepted_true() -> TestCase {
        TestCase::new(
            "gc_params_flap_present_accepted_true",
            "After the EdgionGatewayConfig is created, GatewayClass should become Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_cluster_resource_status(&ctx, "GatewayClass", GC).await {
                        Ok(gc) => {
                            if helpers::has_condition(&gc, "Accepted", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=True as expected (parametersRef resolved)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&gc["status"]).unwrap_or_default()
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
impl TestSuite for GcParamsFlapPresentTestSuite {
    fn name(&self) -> &str {
        "GcParamsFlapPresent"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_resolved_refs_true(), Self::test_accepted_true()]
    }
}

// ---------------------------------------------------------------------------
// Unresolved phase suite (EGC exists but declares a missing global plugin)
// ---------------------------------------------------------------------------

pub struct GcParamsFlapUnresolvedTestSuite;

impl GcParamsFlapUnresolvedTestSuite {
    fn test_resolved_refs_false_but_accepted() -> TestCase {
        TestCase::new(
            "gc_params_flap_unresolved_reports_missing_plugin",
            "A missing EGC global plugin should give GatewayClass ResolvedRefs=False while it stays Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_cluster_resource_status(&ctx, "GatewayClass", GC).await {
                        Ok(gc) => {
                            let status = serde_json::to_string(&gc["status"]).unwrap_or_default();
                            if helpers::has_condition(&gc, "ResolvedRefs", "False")
                                && helpers::has_condition(&gc, "Accepted", "True")
                                && status.contains("edgion-test/params-flap-missing-plugin")
                            {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False naming the missing plugin, Accepted=True".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False naming the missing plugin and Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&gc["status"]).unwrap_or_default()
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
impl TestSuite for GcParamsFlapUnresolvedTestSuite {
    fn name(&self) -> &str {
        "GcParamsFlapUnresolved"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_resolved_refs_false_but_accepted()]
    }
}
