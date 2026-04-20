// PluginMetaData Status Integration Test Suite
//
// Verifies the Accepted and ResolvedRefs status conditions on PluginMetaData resources.
//
// Required conf files (examples/test/conf/EdgionPlugins/PluginMetaDataStatus/):
//   01_PluginMetaData_pm-status-gcir-valid.yaml        — valid GCIR PM
//   02_PluginMetaData_pm-status-gcir-invalid.yaml      — GCIR PM with bad activeProfile
//   03_PluginMetaData_pm-status-cluster-valid.yaml     — valid ClusterRegionRoute PM
//   04_PluginMetaData_pm-status-service-good-ref.yaml  — ServiceRegionRoute w/ valid clusterRef
//   05_PluginMetaData_pm-status-service-missing-ref.yaml — ServiceRegionRoute w/ missing clusterRef
//   06_PluginMetaData_pm-status-service-wrong-type-ref.yaml — ServiceRegionRoute w/ wrong-type ref
//
// Scenarios:
//   1. valid GCIR PM          → Accepted=True, ResolvedRefs=True
//   2. invalid GCIR PM        → Accepted=False/Invalid, message contains "activeProfile"
//   3. ClusterRegionRoute PM  → Accepted=True, ResolvedRefs=True
//   4. ServiceRegionRoute, valid clusterRef  → Accepted=True, ResolvedRefs=True
//   5. ServiceRegionRoute, missing clusterRef → Accepted=True, ResolvedRefs=False/InvalidPluginMetaDataRef
//   6. ServiceRegionRoute, wrong-type ref    → Accepted=True, ResolvedRefs=False/InvalidPluginMetaDataRef

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const NS: &str = "edgion-test";

pub struct PluginMetaDataStatusTestSuite;

impl PluginMetaDataStatusTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PluginMetaDataStatusTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helper: poll the configserver API for a PluginMetaData and return its status
// conditions, retrying up to 20 times with 500ms delay.
// ---------------------------------------------------------------------------

/// Fetch status.conditions for a PluginMetaData from the configserver API.
/// Returns Ok(conditions_array) on success or Err(last_error_message) after 20 retries.
async fn fetch_conditions(ctx: &TestContext, name: &str) -> Result<Vec<serde_json::Value>, String> {
    let url = format!(
        "{}/configserver/PluginMetaData?namespace={}&name={}",
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

impl PluginMetaDataStatusTestSuite {
    // =========================================================================
    // Scenario 1: valid GCIR PM → Accepted=True, ResolvedRefs=True
    // =========================================================================

    fn pm_status_gcir_valid() -> TestCase {
        TestCase::new(
            "pm_status_gcir_valid",
            "GCIR PM 'pm-status-gcir-valid': Accepted=True and ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "pm-status-gcir-valid";

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
    // Scenario 2: invalid GCIR PM (activeProfile not in profiles) → Accepted=False/Invalid
    // =========================================================================

    fn pm_status_gcir_invalid() -> TestCase {
        TestCase::new(
            "pm_status_gcir_invalid",
            "GCIR PM 'pm-status-gcir-invalid': Accepted=False/Invalid with message about activeProfile",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "pm-status-gcir-invalid";

                    // Poll until Accepted=False appears (may take a few cycles)
                    let url = format!(
                        "{}/configserver/PluginMetaData?namespace={}&name={}",
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
                                "Expected Accepted=False/Invalid for invalid GCIR PM. Last: {}",
                                last_err
                            ),
                        )
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 3: ClusterRegionRoute PM → Accepted=True, ResolvedRefs=True
    // =========================================================================

    fn pm_status_cluster_valid() -> TestCase {
        TestCase::new(
            "pm_status_cluster_valid",
            "ClusterRegionRoute PM 'pm-status-cluster-valid': Accepted=True and ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "pm-status-cluster-valid";

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
                                        "ClusterRegionRoute: Accepted=True, ResolvedRefs=True".to_string(),
                                    )
                                }
                                _ => {
                                    TestResult::failed(start.elapsed(), format!("Missing conditions: {:?}", conditions))
                                }
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 4: ServiceRegionRoute with valid clusterRef → Accepted=True, ResolvedRefs=True
    // =========================================================================

    fn pm_status_service_good_ref() -> TestCase {
        TestCase::new(
            "pm_status_service_good_ref",
            "ServiceRegionRoute PM 'pm-status-service-good-ref': ResolvedRefs=True (valid clusterRef)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "pm-status-service-good-ref";

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
                                            format!("ResolvedRefs not True (valid clusterRef): {:?}", r),
                                        );
                                    }
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "ServiceRegionRoute with valid clusterRef: Accepted=True, ResolvedRefs=True"
                                            .to_string(),
                                    )
                                }
                                _ => {
                                    TestResult::failed(start.elapsed(), format!("Missing conditions: {:?}", conditions))
                                }
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 5: ServiceRegionRoute, clusterRef points at non-existent PM
    //             → Accepted=True, ResolvedRefs=False/InvalidPluginMetaDataRef
    // =========================================================================

    fn pm_status_service_missing_ref() -> TestCase {
        TestCase::new(
            "pm_status_service_missing_ref",
            "ServiceRegionRoute PM 'pm-status-service-missing-ref': ResolvedRefs=False/InvalidPluginMetaDataRef",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "pm-status-service-missing-ref";

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
                                            format!("Accepted should be True: {:?}", a),
                                        );
                                    }
                                    if r["status"].as_str() != Some("False") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("ResolvedRefs should be False: {:?}", r),
                                        );
                                    }
                                    if r["reason"].as_str() != Some("InvalidPluginMetaDataRef") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("ResolvedRefs reason should be InvalidPluginMetaDataRef: {:?}", r),
                                        );
                                    }
                                    let msg = r["message"].as_str().unwrap_or("");
                                    if !msg.contains("pm-status-does-not-exist") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("ResolvedRefs message should contain PM name: '{}'", msg),
                                        );
                                    }
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "ResolvedRefs=False/InvalidPluginMetaDataRef for missing clusterRef"
                                            .to_string(),
                                    )
                                }
                                _ => {
                                    TestResult::failed(start.elapsed(), format!("Missing conditions: {:?}", conditions))
                                }
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 6: ServiceRegionRoute, clusterRef points at existing but wrong-type PM
    //             → ResolvedRefs=False/InvalidPluginMetaDataRef
    // =========================================================================

    fn pm_status_service_wrong_type_ref() -> TestCase {
        TestCase::new(
            "pm_status_service_wrong_type_ref",
            "ServiceRegionRoute PM 'pm-status-service-wrong-type-ref': ResolvedRefs=False when ref points at wrong type",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "pm-status-service-wrong-type-ref";

                    match fetch_conditions(&ctx, name).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => {
                            let resolved = find_condition(&conditions, "ResolvedRefs");

                            match resolved {
                                Some(r) => {
                                    if r["status"].as_str() != Some("False") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("ResolvedRefs should be False (wrong-type ref): {:?}", r),
                                        );
                                    }
                                    if r["reason"].as_str() != Some("InvalidPluginMetaDataRef") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "ResolvedRefs reason should be InvalidPluginMetaDataRef: {:?}",
                                                r
                                            ),
                                        );
                                    }
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "ResolvedRefs=False/InvalidPluginMetaDataRef for wrong-type clusterRef"
                                            .to_string(),
                                    )
                                }
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!("ResolvedRefs condition not found: {:?}", conditions),
                                ),
                            }
                        }
                    }
                })
            },
        )
    }
}

impl TestSuite for PluginMetaDataStatusTestSuite {
    fn name(&self) -> &str {
        "PluginMetaDataStatus"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::pm_status_gcir_valid(),
            Self::pm_status_gcir_invalid(),
            Self::pm_status_cluster_valid(),
            Self::pm_status_service_good_ref(),
            Self::pm_status_service_missing_ref(),
            Self::pm_status_service_wrong_type_ref(),
        ]
    }
}
