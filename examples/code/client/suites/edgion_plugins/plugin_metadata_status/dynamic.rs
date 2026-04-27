//! PMD RefFlap scenario phase suites.
//!
//! Initial phase: EdgionPlugins references existing ServiceRegionRoute PMD `pmd-flap-good`
//!                → Accepted=True.
//! Invalid phase: EdgionPlugins references non-existent `pmd-flap-missing`
//!                → Accepted=False (reason: Invalid).
//! Valid phase:   EdgionPlugins restored to reference `pmd-flap-good`
//!                → Accepted=True (recovered).

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

const NS: &str = "edgion-test";
const EP: &str = "pmd-flap-plugin";

// ---------------------------------------------------------------------------
// Helper: fetch status.conditions for the EdgionPlugins under test.
// Returns a single-shot result — no internal retry loop (wait_phase polls).
// ---------------------------------------------------------------------------

async fn fetch_ep_accepted(ctx: &TestContext) -> Result<(String, String), String> {
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

    let accepted = conditions
        .iter()
        .find(|c| c["type"].as_str() == Some("Accepted"))
        .ok_or_else(|| format!("EP '{}' has no Accepted condition", EP))?;

    let status = accepted["status"].as_str().unwrap_or("").to_string();
    let reason = accepted["reason"].as_str().unwrap_or("").to_string();

    Ok((status, reason))
}

// ---------------------------------------------------------------------------
// Initial phase suite
// ---------------------------------------------------------------------------

pub struct PmdFlapInitialTestSuite;

impl PmdFlapInitialTestSuite {
    fn test_accepted_true() -> TestCase {
        TestCase::new(
            "pmd_flap_initial_accepted_true",
            "EdgionPlugins referencing existing pmd-flap-good PMD should be Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_ep_accepted(&ctx).await {
                        Ok((status, _reason)) if status == "True" => TestResult::passed_with_message(
                            start.elapsed(),
                            "Accepted=True as expected (pmd-flap-good PMD present)".to_string(),
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
impl TestSuite for PmdFlapInitialTestSuite {
    fn name(&self) -> &str {
        "PmdFlapInitial"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_accepted_true()]
    }
}

// ---------------------------------------------------------------------------
// Invalid phase suite (polled by wait_phase)
// ---------------------------------------------------------------------------

pub struct PmdFlapInvalidTestSuite;

impl PmdFlapInvalidTestSuite {
    fn test_accepted_false() -> TestCase {
        TestCase::new(
            "pmd_flap_invalid_accepted_false",
            "EdgionPlugins referencing missing pmd-flap-missing PMD should be Accepted=False (reason: Invalid)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_ep_accepted(&ctx).await {
                        Ok((status, reason)) if status == "False" && reason == "Invalid" => {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Accepted=False/Invalid as expected (pmd-flap-missing PMD not found)".to_string(),
                            )
                        }
                        Ok((status, reason)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected Accepted=False/Invalid, got status={} reason={}",
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
impl TestSuite for PmdFlapInvalidTestSuite {
    fn name(&self) -> &str {
        "PmdFlapInvalid"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_accepted_false()]
    }
}

// ---------------------------------------------------------------------------
// Valid (recovered) phase suite (polled by wait_phase)
// ---------------------------------------------------------------------------

pub struct PmdFlapValidTestSuite;

impl PmdFlapValidTestSuite {
    fn test_accepted_true_recovered() -> TestCase {
        TestCase::new(
            "pmd_flap_valid_accepted_true_recovered",
            "EdgionPlugins restored to pmd-flap-good PMD should be Accepted=True again",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_ep_accepted(&ctx).await {
                        Ok((status, _reason)) if status == "True" => TestResult::passed_with_message(
                            start.elapsed(),
                            "Accepted=True as expected (recovered to pmd-flap-good)".to_string(),
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
}

#[async_trait]
impl TestSuite for PmdFlapValidTestSuite {
    fn name(&self) -> &str {
        "PmdFlapValid"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_accepted_true_recovered()]
    }
}
