// EdgionDataManagedRegionRoute Integration Test Suite
//
// Tests the EdgionData overlay architecture where RegionRouteOverride EdgionDatas are pre-created
// by administrators and referenced via EdgionPlugins.overrideRef.
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/plugin-metadata-managed/):
//   01_EdgionData_pm-managed.yaml          — RegionRouteOverride EdgionData for pm-managed test
//   01_EdgionData_pm-validated.yaml        — RegionRouteOverride EdgionData for pm-validated test
//   01_EdgionPlugins_pm-managed.yaml           — EdgionPlugins with overrideRef → pm-managed
//   01_EdgionPlugins_pm-validated.yaml         — EdgionPlugins with overrideRef → pm-validated
//   02_HTTPRoute_pm-managed-test.yaml
//
// Scenarios:
//   1. override_data_exists: Verify that the pre-created RegionRouteOverride EdgionData "pm-managed"
//      exists with correct regions via the configserver API.
//   2. ep_resolves_pm_chain: Verify that EdgionPlugins "pm-managed" is Accepted (overrideRef resolves).
//   3. pm_validated_accepted: Verify that EdgionPlugins "pm-validated" is also accepted
//      when its pre-created RegionRouteOverride EdgionData is present.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const PM_MANAGED_NS: &str = "edgion-default";

pub struct EdgionDataManagedTestSuite;

impl EdgionDataManagedTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionDataManagedTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgionDataManagedTestSuite {
    // =========================================================================
    // Test 1: override_data_exists
    // =========================================================================

    /// Verify that the pre-created RegionRouteOverride EdgionData "pm-managed" exists with the correct
    /// regions. Administrators must create this EdgionData before the EP is applied.
    fn override_data_exists() -> TestCase {
        TestCase::new(
            "override_data_exists",
            "Pre-created RegionRouteOverride EdgionData 'pm-managed' exists with correct regions",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "{}/configserver/EdgionData?namespace={}&name=pm-managed",
                        ctx.admin_api_url(),
                        PM_MANAGED_NS
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
                                    let data = &body["data"];
                                    let ed_data = &data["spec"]["data"];
                                    let ed_type = ed_data["type"].as_str().unwrap_or("");
                                    if ed_type == "RegionRouteOverride" {
                                        let config = &ed_data["config"];
                                        let regions = &config["regions"];
                                        let has_regions = regions.as_array().map(|r| !r.is_empty()).unwrap_or(false);

                                        if has_regions {
                                            found = true;
                                            break;
                                        } else {
                                            last_err =
                                                "RegionRouteOverride found but regions list is empty".to_string();
                                        }
                                    } else {
                                        last_err = format!(
                                            "EdgionData found but data type is not RegionRouteOverride: {}",
                                            ed_type
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
                            "RegionRouteOverride EdgionData 'pm-managed' exists with correct regions".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "RegionRouteOverride EdgionData 'pm-managed' not found or invalid within timeout. Last error: {}",
                                last_err
                            ),
                        )
                    }
                })
            },
        )
    }

    // =========================================================================
    // Test 2: ep_resolves_pm_chain
    // =========================================================================

    /// Verify that EdgionPlugins "pm-managed" is processed successfully (Accepted condition).
    /// The overrideRef must resolve to the pre-created RegionRouteOverride EdgionData for the EP to be accepted.
    fn ep_resolves_pm_chain() -> TestCase {
        TestCase::new(
            "ep_resolves_pm_chain",
            "EdgionPlugins 'pm-managed' is Accepted after resolving overrideRef to RegionRouteOverride EdgionData",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let ep_url = format!(
                        "{}/configserver/EdgionPlugins?namespace={}&name=pm-managed",
                        ctx.admin_api_url(),
                        PM_MANAGED_NS
                    );

                    let mut accepted = false;
                    let mut last_err = String::new();

                    for _ in 0..20 {
                        let mut req = ctx.http_client.get(&ep_url);
                        if let Some(ref token) = ctx.admin_token {
                            req = req.header("Authorization", format!("Bearer {}", token));
                        }

                        match req.send().await {
                            Ok(resp) if resp.status().is_success() => {
                                match resp.json::<serde_json::Value>().await {
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
                                                    "EdgionPlugins found but Accepted condition not True: {:?}",
                                                    arr
                                                );
                                            }
                                        } else {
                                            // No conditions yet — resource may still be processing
                                            last_err =
                                                "EdgionPlugins found but status.conditions not present yet".to_string();
                                        }
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

                    if accepted {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "EdgionPlugins 'pm-managed' accepted — overrideRef resolved correctly".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "EdgionPlugins 'pm-managed' not Accepted within timeout. Last error: {}",
                                last_err
                            ),
                        )
                    }
                })
            },
        )
    }

    // =========================================================================
    // Test 3: pm_validated_accepted
    // =========================================================================

    /// Verify that EdgionPlugins "pm-validated" is also accepted when its pre-created
    /// RegionRouteOverride EdgionData "pm-validated" is present.
    fn pm_validated_accepted() -> TestCase {
        TestCase::new(
            "pm_validated_accepted",
            "EdgionPlugins 'pm-validated' is Accepted with pre-created Service PM present",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // First ensure the RegionRouteOverride EdgionData "pm-validated" is visible via configserver
                    let pm_url = format!(
                        "{}/configserver/EdgionData?namespace={}&name=pm-validated",
                        ctx.admin_api_url(),
                        PM_MANAGED_NS
                    );
                    let mut pm_exists = false;
                    for _ in 0..20 {
                        let mut req = ctx.http_client.get(&pm_url);
                        if let Some(ref token) = ctx.admin_token {
                            req = req.header("Authorization", format!("Bearer {}", token));
                        }
                        if let Ok(resp) = req.send().await {
                            if resp.status().is_success() {
                                if let Ok(body) = resp.json::<serde_json::Value>().await {
                                    if body["success"].as_bool().unwrap_or(false)
                                        && body["data"]["spec"]["data"]["type"].as_str() == Some("RegionRouteOverride")
                                    {
                                        pm_exists = true;
                                        break;
                                    }
                                }
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                    if !pm_exists {
                        return TestResult::failed(
                            start.elapsed(),
                            "Pre-created RegionRouteOverride EdgionData 'pm-validated' not found via configserver within timeout"
                                .to_string(),
                        );
                    }

                    // Now verify EdgionPlugins "pm-validated" is accepted
                    let ep_url = format!(
                        "{}/configserver/EdgionPlugins?namespace={}&name=pm-validated",
                        ctx.admin_api_url(),
                        PM_MANAGED_NS
                    );

                    let mut accepted = false;
                    let mut last_err = String::new();

                    for _ in 0..20 {
                        let mut req = ctx.http_client.get(&ep_url);
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
                                                "EdgionPlugins found but Accepted condition not True: {:?}",
                                                arr
                                            );
                                        }
                                    } else {
                                        last_err =
                                            "EdgionPlugins found but status.conditions not present yet".to_string();
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
                            "EdgionPlugins 'pm-validated' accepted with pre-created RegionRouteOverride EdgionData"
                                .to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "EdgionPlugins 'pm-validated' not Accepted within timeout. Last error: {}",
                                last_err
                            ),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for EdgionDataManagedTestSuite {
    fn name(&self) -> &str {
        "EdgionDataManagedRegionRoute"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Test 1: Pre-created RegionRouteOverride EdgionData exists with correct structure
            Self::override_data_exists(),
            // Test 2: EdgionPlugins resolves overrideRef and is accepted
            Self::ep_resolves_pm_chain(),
            // Test 3: pm-validated EdgionPlugins is also accepted with its pre-created RegionRouteOverride EdgionData
            Self::pm_validated_accepted(),
        ]
    }
}
