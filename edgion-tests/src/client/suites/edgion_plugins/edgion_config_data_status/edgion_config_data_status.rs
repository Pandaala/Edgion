// EdgionConfigData Status Integration Test Suite
//
// Verifies the Accepted and ResolvedRefs status conditions on EdgionConfigData resources.
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/EdgionConfigDataStatus/):
//   01_EdgionConfigData_data-status-gcir-valid.yaml   — valid Selector EdgionConfigData
//   02_EdgionConfigData_data-status-gcir-invalid.yaml — EdgionStreamPlugins with an invalid
//                                                        inline GCIR config (bad activeProfile)
//
// Scenarios:
//   1. valid Selector EdgionConfigData -> Accepted=True, ResolvedRefs=True
//   2. invalid inline GCIR config (EdgionStreamPlugins) -> Accepted=False/Invalid, message
//      contains "activeProfile"
//
// NOTE: a Selector EdgionConfigData is always structurally valid, so the "invalid" case can no
// longer be an EdgionConfigData. The GCIR rule data now lives inline in the plugin config, so
// an invalid activeProfile is rejected during EdgionStreamPlugins preparse instead.
//
// NOTE: Scenarios 3-6 (ClusterRegionRoute, ServiceRegionRoute, clusterRef chain) were removed
// in the EdgionConfigData overlay refactor (H3). The ClusterRegionRoute and ServiceRegionRoute
// EdgionConfigData variants no longer exist; overrideRef resolution is tracked via EdgionPlugins
// ResolvedRefs conditions, not via EdgionConfigData cross-resource refs.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const NS: &str = "edgion-test";

pub struct EdgionConfigDataStatusTestSuite;

impl EdgionConfigDataStatusTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionConfigDataStatusTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helper: poll the configserver API for a EdgionConfigData and return its status
// conditions, retrying up to 20 times with 500ms delay.
// ---------------------------------------------------------------------------

/// Fetch status.conditions for a EdgionConfigData from the configserver API.
/// Returns Ok(conditions_array) on success or Err(last_error_message) after 20 retries.
async fn fetch_conditions(ctx: &TestContext, name: &str) -> Result<Vec<serde_json::Value>, String> {
    let url = format!(
        "{}/configserver/EdgionConfigData?namespace={}&name={}",
        ctx.admin_api_url(),
        NS,
        name
    );

    let mut last_err = format!("PM '{}' not found", name);

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
                        last_err = format!("PM '{}' found but status.conditions is empty", name);
                    } else {
                        last_err = format!("PM '{}' found but status.conditions not present", name);
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

/// Find a condition by type in the conditions array.
fn find_condition<'a>(conditions: &'a [serde_json::Value], type_: &str) -> Option<&'a serde_json::Value> {
    conditions.iter().find(|c| c["type"].as_str() == Some(type_))
}

impl EdgionConfigDataStatusTestSuite {
    // =========================================================================
    // Scenario 1: valid Selector EdgionConfigData → Accepted=True, ResolvedRefs=True
    // =========================================================================

    fn pm_status_gcir_valid() -> TestCase {
        TestCase::new(
            "pm_status_gcir_valid",
            "Selector EdgionConfigData 'data-status-gcir-valid': Accepted=True and ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "data-status-gcir-valid";

                    match fetch_conditions(&ctx, name).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => {
                            let accepted = find_condition(&conditions, "Accepted");
                            let resolved = find_condition(&conditions, "ResolvedRefs");

                            match (accepted, resolved) {
                                (Some(a), Some(r)) => {
                                    if a["status"].as_str() != Some("True") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Accepted not True: {:?}", a),
                                        );
                                    }
                                    if r["status"].as_str() != Some("True") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("ResolvedRefs not True: {:?}", r),
                                        );
                                    }
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Accepted=True, ResolvedRefs=True".to_string(),
                                    )
                                }
                                _ => TestResult::failed(
                                    start.elapsed(),
                                    format!("Missing Accepted or ResolvedRefs condition: {:?}", conditions),
                                ),
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 2: invalid inline GCIR config (activeProfile not in profiles) → Accepted=False/Invalid
    //
    // The invalid config lives in an EdgionStreamPlugins (the GCIR rule data is inline on the
    // plugin now, not on an EdgionConfigData), so this scenario queries EdgionStreamPlugins.
    // =========================================================================

    fn pm_status_gcir_invalid() -> TestCase {
        TestCase::new(
            "pm_status_gcir_invalid",
            "EdgionStreamPlugins 'data-status-gcir-invalid': Accepted=False/Invalid with message about activeProfile",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "data-status-gcir-invalid";

                    // Poll until Accepted=False appears (may take a few cycles)
                    let url = format!(
                        "{}/configserver/EdgionStreamPlugins?namespace={}&name={}",
                        ctx.admin_api_url(),
                        NS,
                        name
                    );

                    let mut accepted_false_found = false;
                    let mut last_err = String::new();

                    for _ in 0..20 {
                        let mut req = ctx.http_client.get(&url);
                        if let Some(ref token) = ctx.admin_token {
                            req = req.header("Authorization", format!("Bearer {}", token));
                        }

                        match req.send().await {
                            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                                    if let Some(conditions) = body["data"]["status"]["conditions"].as_array() {
                                        if let Some(accepted) = find_condition(conditions, "Accepted") {
                                            if accepted["status"].as_str() == Some("False")
                                                && accepted["reason"].as_str() == Some("Invalid")
                                            {
                                                let msg = accepted["message"].as_str().unwrap_or("");
                                                if msg.contains("activeProfile") {
                                                    accepted_false_found = true;
                                                    break;
                                                } else {
                                                    last_err = format!(
                                                            "Accepted=False/Invalid but message lacks 'activeProfile': '{}'",
                                                            msg
                                                        );
                                                }
                                            } else {
                                                last_err =
                                                    format!("Accepted condition not False/Invalid: {:?}", accepted);
                                            }
                                        } else {
                                            last_err = "status.conditions lacks Accepted".to_string();
                                        }
                                    } else {
                                        last_err = "status.conditions not present".to_string();
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

                    if accepted_false_found {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Accepted=False/Invalid with 'activeProfile' in message".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected Accepted=False/Invalid for invalid GCIR EdgionStreamPlugins. Last: {}",
                                last_err
                            ),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for EdgionConfigDataStatusTestSuite {
    fn name(&self) -> &str {
        "EdgionConfigDataStatus"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::pm_status_gcir_valid(), Self::pm_status_gcir_invalid()]
    }
}
