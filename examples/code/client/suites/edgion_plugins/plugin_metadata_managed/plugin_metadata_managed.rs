// PluginMetaDataManagedRegionRoute Integration Test Suite
//
// Tests the new PluginMetaData-managed architecture where ServiceRegionRoute PluginMetaDatas are pre-created by administrators.
// The resolve chain is: EP.serviceRegionRouteRef → ServiceRegionRoute PluginMetaData → ServiceRegionRoute PluginMetaData.clusterRef → ClusterRegionRoute PluginMetaData
//
// Required conf files (examples/test/conf/EdgionPlugins/plugin-metadata-managed/):
//   01_PluginMetaData_pm-managed-global.yaml  — ClusterRegionRoute PluginMetaData (shared by both tests)
//   01_PluginMetaData_pm-managed.yaml          — ServiceRegionRoute PluginMetaData for pm-managed test
//   01_PluginMetaData_pm-validated.yaml        — ServiceRegionRoute PluginMetaData for pm-validated test
//   01_EdgionPlugins_pm-managed.yaml           — EdgionPlugins referencing pm-managed ServiceRegionRoute PluginMetaData
//   01_EdgionPlugins_pm-validated.yaml         — EdgionPlugins referencing pm-validated ServiceRegionRoute PluginMetaData
//   02_HTTPRoute_pm-managed-test.yaml
//
// Scenarios:
//   1. service_pm_exists: Verify that the pre-created ServiceRegionRoute PluginMetaData "pm-managed" exists
//      with correct clusterRef and regions via the configserver API.
//   2. ep_resolves_pm_chain: Verify that EdgionPlugins "pm-managed" is processed successfully
//      (Accepted condition) — the PluginMetaData chain (EP → ServiceRegionRoute PluginMetaData → ClusterRegionRoute PluginMetaData) resolves correctly.
//   3. pm_validated_accepted: Verify that EdgionPlugins "pm-validated" is also accepted
//      when its pre-created ServiceRegionRoute PluginMetaData is present.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const PM_MANAGED_NS: &str = "edgion-default";

pub struct PluginMetaDataManagedTestSuite;

impl PluginMetaDataManagedTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PluginMetaDataManagedTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginMetaDataManagedTestSuite {
    // =========================================================================
    // Test 1: service_pm_exists
    // =========================================================================

    /// Verify that the pre-created ServiceRegionRoute PluginMetaData "pm-managed" exists with the correct
    /// clusterRef and regions. Administrators must create this PluginMetaData before the EP is applied.
    fn service_pm_exists() -> TestCase {
        TestCase::new(
            "service_pm_exists",
            "Pre-created ServiceRegionRoute PM 'pm-managed' exists with correct clusterRef and regions",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "{}/configserver/PluginMetaData?namespace={}&name=pm-managed",
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
                                    let metadata = &data["spec"]["metadata"];
                                    let md_type = metadata["type"].as_str().unwrap_or("");
                                    if md_type == "ServiceRegionRoute" {
                                        let config = &metadata["config"];
                                        let cluster_ref = &config["clusterRef"];
                                        let cluster_name = cluster_ref["name"].as_str().unwrap_or("");
                                        let cluster_ns = cluster_ref["namespace"].as_str().unwrap_or("");
                                        let regions = &config["regions"];
                                        let has_regions = regions.as_array().map(|r| !r.is_empty()).unwrap_or(false);

                                        if cluster_name == "pm-managed-global"
                                            && cluster_ns == PM_MANAGED_NS
                                            && has_regions
                                        {
                                            found = true;
                                            break;
                                        } else {
                                            last_err = format!(
                                                    "ServiceRegionRoute found but clusterRef or regions mismatch: clusterRef.name={}, clusterRef.namespace={}, has_regions={}",
                                                    cluster_name, cluster_ns, has_regions
                                                );
                                        }
                                    } else {
                                        last_err = format!(
                                            "PluginMetaData found but metadata type is not ServiceRegionRoute: {}",
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
                            "ServiceRegionRoute PM 'pm-managed' exists with correct clusterRef and regions".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "ServiceRegionRoute PM 'pm-managed' not found or invalid within timeout. Last error: {}",
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
    /// The PluginMetaData chain (EP.serviceRegionRouteRef → ServiceRegionRoute PluginMetaData → ServiceRegionRoute PluginMetaData.clusterRef → ClusterRegionRoute PluginMetaData)
    /// must resolve correctly for the EP to be accepted.
    fn ep_resolves_pm_chain() -> TestCase {
        TestCase::new(
            "ep_resolves_pm_chain",
            "EdgionPlugins 'pm-managed' is Accepted after resolving the Service PM → Cluster PM chain",
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
                            "EdgionPlugins 'pm-managed' accepted — PM chain resolved correctly".to_string(),
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
    /// ServiceRegionRoute PluginMetaData "pm-validated" is present and references the shared ClusterRegionRoute PluginMetaData.
    fn pm_validated_accepted() -> TestCase {
        TestCase::new(
            "pm_validated_accepted",
            "EdgionPlugins 'pm-validated' is Accepted with pre-created Service PM present",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // First ensure the ServiceRegionRoute PluginMetaData "pm-validated" is visible via configserver
                    let pm_url = format!(
                        "{}/configserver/PluginMetaData?namespace={}&name=pm-validated",
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
                                        && body["data"]["spec"]["metadata"]["type"].as_str()
                                            == Some("ServiceRegionRoute")
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
                            "Pre-created Service PM 'pm-validated' not found via configserver within timeout"
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
                            "EdgionPlugins 'pm-validated' accepted with pre-created Service PM".to_string(),
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

impl TestSuite for PluginMetaDataManagedTestSuite {
    fn name(&self) -> &str {
        "PluginMetaDataManagedRegionRoute"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Test 1: Pre-created ServiceRegionRoute PluginMetaData exists with correct structure
            Self::service_pm_exists(),
            // Test 2: EdgionPlugins resolves the full PluginMetaData chain and is accepted
            Self::ep_resolves_pm_chain(),
            // Test 3: pm-validated EdgionPlugins is also accepted with its pre-created ServiceRegionRoute PluginMetaData
            Self::pm_validated_accepted(),
        ]
    }
}
