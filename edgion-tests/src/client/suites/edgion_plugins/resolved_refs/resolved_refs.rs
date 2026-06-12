// EdgionPlugins / EdgionStreamPlugins ResolvedRefs Status Integration Test Suite
//
// Verifies that the ResolvedRefs condition is correctly set on EP and ESP resources.
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/ResolvedRefs/):
//   01_Secret_edgion-test_ep-resolved-refs-test-secret.yaml         — real Secret
//   02_EdgionPlugins_edgion-test_ep-secret-exists.yaml              — EP refs existing Secret
//   03_EdgionPlugins_edgion-test_ep-secret-missing.yaml             — EP refs missing Secret
//   04_PluginMetaData_edgion-test_esp-gcir-pm-exists.yaml           — GCIR PM (loaded first)
//   05_EdgionStreamPlugins_edgion-test_esp-gcir-pm-exists.yaml      — ESP refs existing PM
//   06_EdgionStreamPlugins_edgion-test_esp-gcir-pm-missing.yaml     — ESP refs missing PM
//
// Scenarios:
//   1. ep_resolved_refs_true_when_secret_exists   — EP + existing Secret → ResolvedRefs=True
//   2. ep_resolved_refs_false_when_secret_missing — EP + missing Secret  → ResolvedRefs=False/SecretNotFound
//   3. esp_resolved_refs_true_when_gcir_pm_exists — ESP + existing GCIR PM → ResolvedRefs=True
//   4. esp_resolved_refs_false_when_gcir_pm_missing — ESP + missing PM → ResolvedRefs=False/InvalidPluginMetaDataRef

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const NS: &str = "edgion-test";

pub struct ResolvedRefsTestSuite;

impl ResolvedRefsTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ResolvedRefsTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetch status.conditions for an EdgionPlugins resource.
async fn fetch_ep_conditions(ctx: &TestContext, name: &str) -> Result<Vec<serde_json::Value>, String> {
    fetch_conditions(ctx, "EdgionPlugins", name).await
}

/// Fetch status.conditions for an EdgionStreamPlugins resource.
async fn fetch_esp_conditions(ctx: &TestContext, name: &str) -> Result<Vec<serde_json::Value>, String> {
    fetch_conditions(ctx, "EdgionStreamPlugins", name).await
}

async fn fetch_conditions(ctx: &TestContext, kind: &str, name: &str) -> Result<Vec<serde_json::Value>, String> {
    let url = format!(
        "{}/configserver/{}?namespace={}&name={}",
        ctx.admin_api_url(),
        kind,
        NS,
        name
    );

    let mut last_err = format!("{} '{}' not found", kind, name);

    for _ in 0..20 {
        let mut req = ctx.http_client.get(&url);
        if let Some(ref token) = ctx.admin_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                    if let Some(conditions) = body["data"]["status"]["conditions"].as_array() {
                        if !conditions.is_empty() {
                            return Ok(conditions.clone());
                        }
                        last_err = format!("{} '{}' found but status.conditions is empty", kind, name);
                    } else {
                        last_err = format!("{} '{}' found but status.conditions not present", kind, name);
                    }
                }
                Ok(body) => {
                    last_err = format!("API error: {}", body["error"].as_str().unwrap_or("unknown"));
                }
                Err(e) => {
                    last_err = format!("JSON parse error: {}", e);
                }
            },
            Ok(resp) => {
                last_err = format!("HTTP {}", resp.status());
            }
            Err(e) => {
                last_err = format!("Request error: {}", e);
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Err(last_err)
}

fn find_condition<'a>(conditions: &'a [serde_json::Value], type_: &str) -> Option<&'a serde_json::Value> {
    conditions.iter().find(|c| c["type"].as_str() == Some(type_))
}

impl ResolvedRefsTestSuite {
    // =========================================================================
    // Scenario 1: EP referencing an existing Secret → ResolvedRefs=True
    // =========================================================================

    fn ep_resolved_refs_true_when_secret_exists() -> TestCase {
        TestCase::new(
            "ep_resolved_refs_true_when_secret_exists",
            "EdgionPlugins 'ep-secret-exists': ResolvedRefs=True when Secret exists",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "ep-secret-exists";

                    match fetch_ep_conditions(&ctx, name).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => match find_condition(&conditions, "ResolvedRefs") {
                            Some(c) if c["status"].as_str() == Some("True") => {
                                TestResult::passed_with_message(start.elapsed(), "ResolvedRefs=True".to_string())
                            }
                            Some(c) => TestResult::failed(
                                start.elapsed(),
                                format!("ResolvedRefs not True (Secret exists): {:?}", c),
                            ),
                            None => TestResult::failed(
                                start.elapsed(),
                                format!("ResolvedRefs condition not found: {:?}", conditions),
                            ),
                        },
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 2: EP referencing a missing Secret → ResolvedRefs=False/SecretNotFound
    // =========================================================================

    fn ep_resolved_refs_false_when_secret_missing() -> TestCase {
        TestCase::new(
            "ep_resolved_refs_false_when_secret_missing",
            "EdgionPlugins 'ep-secret-missing': ResolvedRefs=False/SecretNotFound when Secret is absent",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "ep-secret-missing";
                    let expected_secret_name = "ep-resolved-refs-nonexistent-secret";

                    match fetch_ep_conditions(&ctx, name).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => match find_condition(&conditions, "ResolvedRefs") {
                            Some(c) => {
                                if c["status"].as_str() != Some("False") {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("ResolvedRefs should be False: {:?}", c),
                                    );
                                }
                                if c["reason"].as_str() != Some("SecretNotFound") {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("ResolvedRefs reason should be SecretNotFound: {:?}", c),
                                    );
                                }
                                let msg = c["message"].as_str().unwrap_or("");
                                if !msg.contains(expected_secret_name) {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "ResolvedRefs message should contain '{}': '{}'",
                                            expected_secret_name, msg
                                        ),
                                    );
                                }
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "ResolvedRefs=False/SecretNotFound with '{}' in message",
                                        expected_secret_name
                                    ),
                                )
                            }
                            None => TestResult::failed(
                                start.elapsed(),
                                format!("ResolvedRefs condition not found: {:?}", conditions),
                            ),
                        },
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 3: ESP referencing an existing GCIR PM → ResolvedRefs=True
    // =========================================================================

    fn esp_resolved_refs_true_when_gcir_pm_exists() -> TestCase {
        TestCase::new(
            "esp_resolved_refs_true_when_gcir_pm_exists",
            "EdgionStreamPlugins 'esp-gcir-pm-exists': ResolvedRefs=True when GCIR PM exists",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "esp-gcir-pm-exists";

                    match fetch_esp_conditions(&ctx, name).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => match find_condition(&conditions, "ResolvedRefs") {
                            Some(c) if c["status"].as_str() == Some("True") => {
                                TestResult::passed_with_message(start.elapsed(), "ResolvedRefs=True".to_string())
                            }
                            Some(c) => TestResult::failed(
                                start.elapsed(),
                                format!("ResolvedRefs not True (GCIR PM exists): {:?}", c),
                            ),
                            None => TestResult::failed(
                                start.elapsed(),
                                format!("ResolvedRefs condition not found: {:?}", conditions),
                            ),
                        },
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 4: ESP referencing a missing GCIR PM → ResolvedRefs=False/InvalidPluginMetaDataRef
    // =========================================================================

    fn esp_resolved_refs_false_when_gcir_pm_missing() -> TestCase {
        TestCase::new(
            "esp_resolved_refs_false_when_gcir_pm_missing",
            "EdgionStreamPlugins 'esp-gcir-pm-missing': ResolvedRefs=False/InvalidPluginMetaDataRef when PM absent",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "esp-gcir-pm-missing";
                    let expected_pm_name = "esp-gcir-pm-nonexistent";

                    match fetch_esp_conditions(&ctx, name).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => match find_condition(&conditions, "ResolvedRefs") {
                            Some(c) => {
                                if c["status"].as_str() != Some("False") {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("ResolvedRefs should be False: {:?}", c),
                                    );
                                }
                                if c["reason"].as_str() != Some("InvalidPluginMetaDataRef") {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("ResolvedRefs reason should be InvalidPluginMetaDataRef: {:?}", c),
                                    );
                                }
                                let msg = c["message"].as_str().unwrap_or("");
                                if !msg.contains(expected_pm_name) {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "ResolvedRefs message should contain '{}': '{}'",
                                            expected_pm_name, msg
                                        ),
                                    );
                                }
                                if !msg.contains("GlobalConnectionIpRestriction") {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "ResolvedRefs message should contain 'GlobalConnectionIpRestriction': '{}'",
                                            msg
                                        ),
                                    );
                                }
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "ResolvedRefs=False/InvalidPluginMetaDataRef with '{}' in message",
                                        expected_pm_name
                                    ),
                                )
                            }
                            None => TestResult::failed(
                                start.elapsed(),
                                format!("ResolvedRefs condition not found: {:?}", conditions),
                            ),
                        },
                    }
                })
            },
        )
    }
}

impl TestSuite for ResolvedRefsTestSuite {
    fn name(&self) -> &str {
        "EdgionPlugins/ResolvedRefs"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::ep_resolved_refs_true_when_secret_exists(),
            Self::ep_resolved_refs_false_when_secret_missing(),
            Self::esp_resolved_refs_true_when_gcir_pm_exists(),
            Self::esp_resolved_refs_false_when_gcir_pm_missing(),
        ]
    }
}
