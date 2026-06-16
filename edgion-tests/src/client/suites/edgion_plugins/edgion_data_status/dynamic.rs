//! RefFlap scenario phase suites.
//!
//! Initial phase: EdgionPlugins has overrideRef pointing at existing RegionRouteOverride `data-flap-good`
//!                → Accepted=True, ResolvedRefs=True.
//! Invalid phase: EdgionPlugins overrideRef switches to non-existent `data-flap-missing`
//!                → Accepted=True (resource is structurally valid),
//!                  ResolvedRefs=False (reason: InvalidEdgionDataRef).
//! Valid phase:   EdgionPlugins restored to overrideRef `data-flap-good`
//!                → Accepted=True, ResolvedRefs=True (recovered).

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

const NS: &str = "edgion-test";
const EP: &str = "data-flap-plugin";

// ---------------------------------------------------------------------------
// Helper: fetch a single condition (status, reason) for the EdgionPlugins
// under test. Returns a single-shot result — no internal retry loop
// (wait_phase polls).
// ---------------------------------------------------------------------------

async fn fetch_ep_condition(ctx: &TestContext, condition_type: &str) -> Result<(String, String), String> {
    let url = format!(
        "{}/configserver/EdgionPlugins?namespace={}&name={}",
        ctx.admin_api_url(),
        NS,
        EP
    );

    let mut req = ctx.http_client.get(&url);
    if let Some(ref token) = ctx.admin_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let resp = req.send().await.map_err(|e| format!("request error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| format!("JSON parse error: {}", e))?;

    if !body["success"].as_bool().unwrap_or(false) {
        return Err(format!("API error: {}", body["error"].as_str().unwrap_or("unknown")));
    }

    let conditions = body["data"]["status"]["conditions"]
        .as_array()
        .ok_or_else(|| format!("EP '{}' status.conditions not present", EP))?;

    let cond = conditions
        .iter()
        .find(|c| c["type"].as_str() == Some(condition_type))
        .ok_or_else(|| format!("EP '{}' has no {} condition", EP, condition_type))?;

    let status = cond["status"].as_str().unwrap_or("").to_string();
    let reason = cond["reason"].as_str().unwrap_or("").to_string();

    Ok((status, reason))
}

// ---------------------------------------------------------------------------
// Initial phase suite
// ---------------------------------------------------------------------------

pub struct EdgionDataFlapInitialTestSuite;

impl EdgionDataFlapInitialTestSuite {
    fn test_accepted_true() -> TestCase {
        TestCase::new(
            "pmd_flap_initial_accepted_true",
            "EdgionPlugins with overrideRef to existing data-flap-good EdgionData should be Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_ep_condition(&ctx, "Accepted").await {
                        Ok((status, _reason)) if status == "True" => TestResult::passed_with_message(
                            start.elapsed(),
                            "Accepted=True as expected (data-flap-good EdgionData present)".to_string(),
                        ),
                        Ok((status, reason)) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected Accepted=True, got status={} reason={}", status, reason),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn test_resolved_refs_true() -> TestCase {
        TestCase::new(
            "pmd_flap_initial_resolved_refs_true",
            "EdgionPlugins with overrideRef to existing data-flap-good EdgionData should be ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_ep_condition(&ctx, "ResolvedRefs").await {
                        Ok((status, _reason)) if status == "True" => TestResult::passed_with_message(
                            start.elapsed(),
                            "ResolvedRefs=True as expected (data-flap-good EdgionData resolved)".to_string(),
                        ),
                        Ok((status, reason)) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected ResolvedRefs=True, got status={} reason={}", status, reason),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for EdgionDataFlapInitialTestSuite {
    fn name(&self) -> &str {
        "EdgionDataFlapInitial"
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

pub struct EdgionDataFlapInvalidTestSuite;

impl EdgionDataFlapInvalidTestSuite {
    fn test_resolved_refs_false() -> TestCase {
        TestCase::new(
            "pmd_flap_invalid_resolved_refs_false",
            "EdgionPlugins with overrideRef to missing data-flap-missing EdgionData should be \
             ResolvedRefs=False (reason: InvalidEdgionDataRef)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_ep_condition(&ctx, "ResolvedRefs").await {
                        Ok((status, reason)) if status == "False" && reason == "InvalidEdgionDataRef" => {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "ResolvedRefs=False/InvalidEdgionDataRef as expected \
                                 (data-flap-missing EdgionData not found)"
                                    .to_string(),
                            )
                        }
                        Ok((status, reason)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected ResolvedRefs=False/InvalidEdgionDataRef, \
                                 got status={} reason={}",
                                status, reason
                            ),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn test_accepted_still_true() -> TestCase {
        TestCase::new(
            "pmd_flap_invalid_accepted_true",
            "EdgionPlugins is structurally valid even when its overrideRef is missing — \
             Accepted should remain True; the missing ref surfaces only via ResolvedRefs",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_ep_condition(&ctx, "Accepted").await {
                        Ok((status, _reason)) if status == "True" => TestResult::passed_with_message(
                            start.elapsed(),
                            "Accepted=True as expected (resource itself is structurally valid)".to_string(),
                        ),
                        Ok((status, reason)) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected Accepted=True, got status={} reason={}", status, reason),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for EdgionDataFlapInvalidTestSuite {
    fn name(&self) -> &str {
        "EdgionDataFlapInvalid"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_resolved_refs_false(), Self::test_accepted_still_true()]
    }
}

// ---------------------------------------------------------------------------
// Valid (recovered) phase suite (polled by wait_phase)
// ---------------------------------------------------------------------------

pub struct EdgionDataFlapValidTestSuite;

impl EdgionDataFlapValidTestSuite {
    fn test_accepted_true_recovered() -> TestCase {
        TestCase::new(
            "pmd_flap_valid_accepted_true_recovered",
            "EdgionPlugins restored to data-flap-good PMD should be Accepted=True again",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_ep_condition(&ctx, "Accepted").await {
                        Ok((status, _reason)) if status == "True" => TestResult::passed_with_message(
                            start.elapsed(),
                            "Accepted=True as expected (recovered to data-flap-good)".to_string(),
                        ),
                        Ok((status, reason)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected Accepted=True (recovered), got status={} reason={}",
                                status, reason
                            ),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn test_resolved_refs_true_recovered() -> TestCase {
        TestCase::new(
            "pmd_flap_valid_resolved_refs_true_recovered",
            "EdgionPlugins restored to data-flap-good PMD should be ResolvedRefs=True again",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_ep_condition(&ctx, "ResolvedRefs").await {
                        Ok((status, _reason)) if status == "True" => TestResult::passed_with_message(
                            start.elapsed(),
                            "ResolvedRefs=True as expected (recovered to data-flap-good)".to_string(),
                        ),
                        Ok((status, reason)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected ResolvedRefs=True (recovered), got status={} reason={}",
                                status, reason
                            ),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for EdgionDataFlapValidTestSuite {
    fn name(&self) -> &str {
        "EdgionDataFlapValid"
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
