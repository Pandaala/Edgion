// GlobalConnectionIpRestriction PluginMetaData Integration Test Suite
//
// Verifies that PluginMetaData resources with type=GlobalConnectionIpRestriction are correctly
// parsed, stored, and accepted by the controller via the configserver API.
//
// Required conf files (examples/test/conf/EdgionPlugins/GlobalConnectionIpRestriction/):
//   01_PluginMetaData_gcir-smoke.yaml — GCIR PluginMetaData with two profiles (p1 + strict)
//
// Scenarios:
//   1. gcir_pm_exists: Verify the PM is stored via configserver API with correct metadata type
//   2. gcir_pm_accepted: Verify the PM has Accepted=True condition (controller validated it)
//   3. gcir_pm_config_structure: Verify the stored config has expected profiles and fields

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

    /// Verify the GlobalConnectionIpRestriction PM is stored via configserver API
    /// with the correct metadata type.
    fn gcir_pm_exists() -> TestCase {
        TestCase::new(
            "gcir_pm_exists",
            "PluginMetaData 'gcir-smoke' exists with type=GlobalConnectionIpRestriction",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "{}/configserver/PluginMetaData?namespace={}&name={}",
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
                                    let md_type = body["data"]["spec"]["metadata"]["type"].as_str().unwrap_or("");
                                    if md_type == "GlobalConnectionIpRestriction" {
                                        found = true;
                                        break;
                                    } else {
                                        last_err = format!(
                                            "PM found but type is '{}', expected 'GlobalConnectionIpRestriction'",
                                            md_type
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
                            "GlobalConnectionIpRestriction PM stored correctly in configserver".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "PM '{}' not found or wrong type within timeout. Last error: {}",
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

    /// Verify the GlobalConnectionIpRestriction PM has Accepted=True condition,
    /// meaning the controller validated and accepted the resource.
    fn gcir_pm_accepted() -> TestCase {
        TestCase::new(
            "gcir_pm_accepted",
            "PluginMetaData 'gcir-smoke' has Accepted=True condition",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "{}/configserver/PluginMetaData?namespace={}&name={}",
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
                                            last_err = format!("PM found but Accepted condition not True: {:?}", arr);
                                        }
                                    } else {
                                        last_err = "PM found but status.conditions not present yet".to_string();
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
                            "GlobalConnectionIpRestriction PM accepted by controller".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "PM '{}' not Accepted within timeout. Last error: {}",
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

    /// Verify the stored config has the expected profile structure:
    /// - activeProfile = "p1"
    /// - profiles.p1.defaultAction = "deny", has allow list with one entry
    /// - profiles.strict.defaultAction = "allow", has deny list with one entry
    fn gcir_pm_config_structure() -> TestCase {
        TestCase::new(
            "gcir_pm_config_structure",
            "PluginMetaData 'gcir-smoke' config has expected profiles (p1=deny, strict=allow)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "{}/configserver/PluginMetaData?namespace={}&name={}",
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
                                        let config = &body["data"]["spec"]["metadata"]["config"];

                                        // Check activeProfile
                                        let active = config["activeProfile"].as_str().unwrap_or("");
                                        if active != "p1" {
                                            last_err = format!("activeProfile='{}', expected 'p1'", active);
                                            break;
                                        }

                                        // Check profile p1: defaultAction=deny, has allow list
                                        let p1 = &config["profiles"]["p1"];
                                        let p1_action = p1["defaultAction"].as_str().unwrap_or("");
                                        let p1_allow_len = p1["allow"].as_array().map(|a| a.len()).unwrap_or(0);
                                        if p1_action != "deny" || p1_allow_len == 0 {
                                            last_err = format!(
                                                "p1: defaultAction='{}' allow_len={} (expected deny/1+)",
                                                p1_action, p1_allow_len
                                            );
                                            break;
                                        }

                                        // Check profile strict: defaultAction=allow, has deny list
                                        let strict = &config["profiles"]["strict"];
                                        let strict_action = strict["defaultAction"].as_str().unwrap_or("");
                                        let strict_deny_len = strict["deny"].as_array().map(|a| a.len()).unwrap_or(0);
                                        if strict_action != "allow" || strict_deny_len == 0 {
                                            last_err = format!(
                                                "strict: defaultAction='{}' deny_len={} (expected allow/1+)",
                                                strict_action, strict_deny_len
                                            );
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
                            "GCIR config structure correct: p1=deny+allow-list, strict=allow+deny-list".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("GCIR config structure invalid. Last error: {}", last_err),
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
