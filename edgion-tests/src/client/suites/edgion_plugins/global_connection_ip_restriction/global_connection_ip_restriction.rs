// GlobalConnectionIpRestriction Selector EdgionConfigData Integration Test Suite
//
// Verifies that EdgionConfigData resources with type=Selector — the variant referenced by
// the GlobalConnectionIpRestriction stream plugin's activeProfileRef — are correctly parsed,
// stored, and accepted by the controller via the configserver API. The rule data (profiles)
// lives inline in the plugin config; the Selector only names the active profile.
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/GlobalConnectionIpRestriction/):
//   01_EdgionConfigData_gcir-smoke.yaml — Selector EdgionConfigData (active=p1)
//
// Scenarios:
//   1. gcir_pm_exists: Verify the EdgionConfigData is stored with correct type=Selector
//   2. gcir_pm_accepted: Verify the EdgionConfigData has Accepted=True condition
//   3. gcir_pm_config_structure: Verify the stored Selector config has the expected active profile

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const GCIR_NS: &str = "edgion-test";
const GCIR_NAME: &str = "gcir-smoke";

pub struct GlobalConnectionIpRestrictionTestSuite;

impl GlobalConnectionIpRestrictionTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GlobalConnectionIpRestrictionTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalConnectionIpRestrictionTestSuite {
    // =========================================================================
    // Test 1: gcir_pm_exists
    // =========================================================================

    /// Verify the Selector EdgionConfigData is stored via configserver API
    /// with the correct type discriminator.
    fn gcir_pm_exists() -> TestCase {
        TestCase::new(
            "gcir_pm_exists",
            "EdgionConfigData 'gcir-smoke' exists with type=Selector",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "{}/configserver/EdgionConfigData?namespace={}&name={}",
                        ctx.admin_api_url(),
                        GCIR_NS,
                        GCIR_NAME
                    );

                    let mut found = false;
                    let mut last_err = String::new();

                    for _ in 0..20 {
                        let mut req = ctx.http_client.get(&url);
                        if let Some(ref token) = ctx.admin_token {
                            req = req.header("Authorization", format!("Bearer {}", token));
                        }

                        match req.send().await {
                            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                                    // spec field is `data` (not `metadata`).
                                    let data_type = body["data"]["spec"]["data"]["type"].as_str().unwrap_or("");
                                    if data_type == "Selector" {
                                        found = true;
                                        break;
                                    } else {
                                        last_err = format!(
                                            "EdgionConfigData found but type is '{}', expected 'Selector'",
                                            data_type
                                        );
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

                    if found {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Selector EdgionConfigData stored correctly in configserver".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "EdgionConfigData '{}' not found or wrong type within timeout. Last error: {}",
                                GCIR_NAME, last_err
                            ),
                        )
                    }
                })
            },
        )
    }

    // =========================================================================
    // Test 2: gcir_pm_accepted
    // =========================================================================

    /// Verify the EdgionConfigData has Accepted=True condition from the controller.
    fn gcir_pm_accepted() -> TestCase {
        TestCase::new(
            "gcir_pm_accepted",
            "EdgionConfigData 'gcir-smoke' has Accepted=True condition",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "{}/configserver/EdgionConfigData?namespace={}&name={}",
                        ctx.admin_api_url(),
                        GCIR_NS,
                        GCIR_NAME
                    );

                    let mut accepted = false;
                    let mut last_err = String::new();

                    for _ in 0..20 {
                        let mut req = ctx.http_client.get(&url);
                        if let Some(ref token) = ctx.admin_token {
                            req = req.header("Authorization", format!("Bearer {}", token));
                        }

                        match req.send().await {
                            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                                    let conditions = &body["data"]["status"]["conditions"];
                                    if let Some(arr) = conditions.as_array() {
                                        let is_accepted = arr.iter().any(|c| {
                                            c["type"].as_str() == Some("Accepted")
                                                && c["status"].as_str() == Some("True")
                                        });
                                        if is_accepted {
                                            accepted = true;
                                            break;
                                        } else {
                                            last_err = format!(
                                                "EdgionConfigData found but Accepted condition not True: {:?}",
                                                arr
                                            );
                                        }
                                    } else {
                                        last_err =
                                            "EdgionConfigData found but status.conditions not present yet".to_string();
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

                    if accepted {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Selector EdgionConfigData accepted by controller".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "EdgionConfigData '{}' not Accepted within timeout. Last error: {}",
                                GCIR_NAME, last_err
                            ),
                        )
                    }
                })
            },
        )
    }

    // =========================================================================
    // Test 3: gcir_pm_config_structure
    // =========================================================================

    /// Verify the stored Selector config has the expected structure:
    /// - active = "p1"
    /// - description present (informational, non-empty)
    fn gcir_pm_config_structure() -> TestCase {
        TestCase::new(
            "gcir_pm_config_structure",
            "EdgionConfigData 'gcir-smoke' Selector config has expected active profile (active=p1)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "{}/configserver/EdgionConfigData?namespace={}&name={}",
                        ctx.admin_api_url(),
                        GCIR_NS,
                        GCIR_NAME
                    );

                    let mut valid = false;
                    let mut last_err = String::new();

                    for _ in 0..20 {
                        let mut req = ctx.http_client.get(&url);
                        if let Some(ref token) = ctx.admin_token {
                            req = req.header("Authorization", format!("Bearer {}", token));
                        }

                        match req.send().await {
                            Ok(resp) if resp.status().is_success() => {
                                match resp.json::<serde_json::Value>().await {
                                    Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                                        // spec.data.config holds the Selector payload.
                                        let config = &body["data"]["spec"]["data"]["config"];

                                        // Check active profile name.
                                        let active = config["active"].as_str().unwrap_or("");
                                        if active != "p1" {
                                            last_err = format!("active='{}', expected 'p1'", active);
                                            break;
                                        }

                                        // Check the informational description survived round-trip.
                                        let description = config["description"].as_str().unwrap_or("");
                                        if description.is_empty() {
                                            last_err = "description missing or empty (expected non-empty)".to_string();
                                            break;
                                        }

                                        valid = true;
                                        break;
                                    }
                                    Ok(body) => {
                                        last_err =
                                            format!("API error: {}", body["error"].as_str().unwrap_or("unknown"));
                                    }
                                    Err(e) => {
                                        last_err = format!("JSON parse error: {}", e);
                                    }
                                }
                            }
                            Ok(resp) => {
                                last_err = format!("HTTP {}", resp.status());
                            }
                            Err(e) => {
                                last_err = format!("Request error: {}", e);
                            }
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }

                    if valid {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "GCIR Selector config structure correct: active=p1 with description".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("GCIR Selector config structure invalid. Last error: {}", last_err),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for GlobalConnectionIpRestrictionTestSuite {
    fn name(&self) -> &str {
        "GlobalConnectionIpRestriction"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::gcir_pm_exists(),
            Self::gcir_pm_accepted(),
            Self::gcir_pm_config_structure(),
        ]
    }
}
