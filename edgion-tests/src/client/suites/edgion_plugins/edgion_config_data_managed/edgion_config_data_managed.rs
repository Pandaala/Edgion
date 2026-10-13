// EdgionConfigDataManagedRegionRoute Integration Test Suite
//
// Tests the EdgionConfigData overlay architecture where RegionRouteOverride EdgionConfigDatas are pre-created
// by administrators and referenced via EdgionPlugins.overrideRef.
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/EdgionConfigDataManaged/):
//   01_EdgionConfigData_data-managed.yaml          — RegionRouteOverride EdgionConfigData for data-managed test
//   01_EdgionConfigData_data-validated.yaml        — RegionRouteOverride EdgionConfigData for data-validated test
//   01_EdgionPlugins_data-managed.yaml           — EdgionPlugins with overrideRef → data-managed
//   01_EdgionPlugins_data-validated.yaml         — EdgionPlugins with overrideRef → data-validated
//   02_HTTPRoute_data-managed-test.yaml
//
// Scenarios:
//   1. override_data_exists: Verify that the pre-created RegionRouteOverride EdgionConfigData "data-managed"
//      exists with correct regions via the configserver API.
//   2. ep_resolves_pm_chain: Verify that EdgionPlugins "data-managed" has current Accepted and ResolvedRefs observations for its overrideRef.
//   3. pm_validated_accepted: Verify that EdgionPlugins "data-validated" also resolves its declared override
//      when its pre-created RegionRouteOverride EdgionConfigData is present.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers::controller_local_conditions;
use std::time::Instant;

const PM_MANAGED_NS: &str = "edgion-default";

fn validate_overlay_resolution(resource: &serde_json::Value, expected: &str) -> Result<(), &'static str> {
    if resource["metadata"]["namespace"] != PM_MANAGED_NS || resource["metadata"]["name"] != expected {
        return Err("Unexpected EdgionPlugins identity");
    }
    let entries = resource["spec"]["requestPlugins"]
        .as_array()
        .ok_or("Missing request plugins")?;
    let mut regions = entries.iter().filter(|entry| entry["type"] == "RegionRoute");
    let entry = regions.next().ok_or("Missing RegionRoute entry")?;
    if regions.next().is_some() {
        return Err("Ambiguous RegionRoute entries");
    }
    match entry.get("enable") {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(true)) => {}
        _ => return Err("RegionRoute entry is not enabled"),
    }
    let reference = &entry["config"]["overrideRef"];
    let namespace = match reference.get("namespace") {
        None | Some(serde_json::Value::Null) => PM_MANAGED_NS,
        Some(value) => value.as_str().ok_or("Invalid override namespace")?,
    };
    if reference["name"] != expected || namespace != PM_MANAGED_NS {
        return Err("Unexpected RegionRoute override reference");
    }
    let generation = match resource["metadata"].get("generation") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(value.as_i64().ok_or("Invalid resource generation")?),
    };
    let conditions = controller_local_conditions(&resource["status"]).ok_or("Missing Controller conditions")?;
    for kind in ["Accepted", "ResolvedRefs"] {
        let mut matching = conditions.iter().filter(|condition| condition["type"] == kind);
        let condition = matching.next().ok_or("Missing Accepted or ResolvedRefs condition")?;
        if matching.next().is_some() || condition["status"] != "True" {
            return Err("Accepted and ResolvedRefs must each be True");
        }
        if generation.is_some_and(|value| condition["observedGeneration"].as_i64() != Some(value)) {
            return Err("Stale Accepted or ResolvedRefs observation");
        }
    }
    Ok(())
}

pub struct EdgionConfigDataManagedTestSuite;

impl EdgionConfigDataManagedTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionConfigDataManagedTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgionConfigDataManagedTestSuite {
    // =========================================================================
    // Test 1: override_data_exists
    // =========================================================================

    /// Verify that the pre-created RegionRouteOverride EdgionConfigData "data-managed" exists with the correct
    /// regions. Administrators must create this EdgionConfigData before the EP is applied.
    fn override_data_exists() -> TestCase {
        TestCase::new(
            "override_data_exists",
            "Pre-created RegionRouteOverride EdgionConfigData 'data-managed' exists with correct regions",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "{}/configserver/EdgionConfigData?namespace={}&name=data-managed",
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
                                            "EdgionConfigData found but data type is not RegionRouteOverride: {}",
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
                            "RegionRouteOverride EdgionConfigData 'data-managed' exists with correct regions"
                                .to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "RegionRouteOverride EdgionConfigData 'data-managed' not found or invalid within timeout. Last error: {}",
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

    /// Verify the declared override edge and current Accepted/ResolvedRefs observations.
    /// Structural acceptance alone permits a valid base with an unresolved overlay.
    fn ep_resolves_pm_chain() -> TestCase {
        TestCase::new(
            "ep_resolves_pm_chain",
            "EdgionPlugins 'data-managed' has Accepted=True and ResolvedRefs=True for its declared override",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let ep_url = format!(
                        "{}/configserver/EdgionPlugins?namespace={}&name=data-managed",
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
                                    match validate_overlay_resolution(&body["data"], "data-managed") {
                                        Ok(()) => {
                                            accepted = true;
                                            break;
                                        }
                                        Err(error) => last_err = error.to_string(),
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
                            "EdgionPlugins 'data-managed' accepted — overrideRef resolved correctly".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "EdgionPlugins 'data-managed' override not resolved within timeout. Last error: {}",
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

    /// Verify that EdgionPlugins "data-validated" also resolves its declared override when its pre-created
    /// RegionRouteOverride EdgionConfigData "data-validated" is present.
    fn pm_validated_accepted() -> TestCase {
        TestCase::new(
            "pm_validated_accepted",
            "EdgionPlugins 'data-validated' has Accepted=True and ResolvedRefs=True for its declared override",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // First ensure the RegionRouteOverride EdgionConfigData "data-validated" is visible via configserver
                    let pm_url = format!(
                        "{}/configserver/EdgionConfigData?namespace={}&name=data-validated",
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
                            "Pre-created RegionRouteOverride EdgionConfigData 'data-validated' not found via configserver within timeout"
                                .to_string(),
                        );
                    }

                    // Now verify EdgionPlugins "data-validated" is accepted
                    let ep_url = format!(
                        "{}/configserver/EdgionPlugins?namespace={}&name=data-validated",
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
                                    match validate_overlay_resolution(&body["data"], "data-validated") {
                                        Ok(()) => {
                                            accepted = true;
                                            break;
                                        }
                                        Err(error) => last_err = error.to_string(),
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
                            "EdgionPlugins 'data-validated' accepted with pre-created RegionRouteOverride EdgionConfigData"
                                .to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "EdgionPlugins 'data-validated' override not resolved within timeout. Last error: {}",
                                last_err
                            ),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for EdgionConfigDataManagedTestSuite {
    fn name(&self) -> &str {
        "EdgionConfigDataManagedRegionRoute"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Test 1: Pre-created RegionRouteOverride EdgionConfigData exists with correct structure
            Self::override_data_exists(),
            // Test 2: EdgionPlugins resolves overrideRef and is accepted
            Self::ep_resolves_pm_chain(),
            // Test 3: data-validated EdgionPlugins also resolves its declared override with its pre-created RegionRouteOverride EdgionConfigData
            Self::pm_validated_accepted(),
        ]
    }
}

#[cfg(test)]
mod overlay_resolution_tests {
    use super::*;
    use serde_json::json;

    fn healthy() -> serde_json::Value {
        json!({"metadata": {"namespace": PM_MANAGED_NS, "name": "data-managed"},
            "spec": {"requestPlugins": [{"type": "RegionRoute", "config": {"overrideRef": {"name": "data-managed"}}}]},
            "status": {"conditions": [{"type": "Accepted", "status": "True"}, {"type": "ResolvedRefs", "status": "True"}]}})
    }

    #[test]
    fn overlay_resolution_accepts_native_and_current_owned_observations() {
        let mut resource = healthy();
        assert!(validate_overlay_resolution(&resource, "data-managed").is_ok());
        resource["metadata"]["generation"] = json!(3);
        for condition in resource["status"]["conditions"].as_array_mut().unwrap() {
            condition["observedGeneration"] = json!(3);
        }
        let native = resource["status"].clone();
        resource["status"] = json!({"controllers": [
            {"controllerName": "foreign/controller", "status": {"conditions": []}},
            {"controllerName": "edgion.io/gateway-controller", "status": native}
        ]});
        assert!(validate_overlay_resolution(&resource, "data-managed").is_ok());
        resource["status"]["controllers"][1]["status"]["conditions"][1]["observedGeneration"] = json!(2);
        assert!(validate_overlay_resolution(&resource, "data-managed").is_err());
    }

    #[test]
    fn overlay_resolution_rejects_accepted_but_unproved_edges() {
        for (pointer, value) in [
            ("/status/conditions/1/status", json!("False")),
            ("/status/conditions/0/status", json!("Unknown")),
            ("/status/conditions", json!([])),
            (
                "/spec/requestPlugins/0/config/overrideRef/name",
                json!("missing-target"),
            ),
            ("/metadata/namespace", json!("wrong-namespace")),
        ] {
            let mut resource = healthy();
            *resource.pointer_mut(pointer).unwrap() = value;
            assert!(validate_overlay_resolution(&resource, "data-managed").is_err());
        }
        let mut resource = healthy();
        resource["spec"]["requestPlugins"][0]["enable"] = json!(false);
        assert!(validate_overlay_resolution(&resource, "data-managed").is_err());
        let mut resource = healthy();
        resource["spec"]["requestPlugins"][0]["config"]["overrideRef"]["namespace"] = json!(17);
        assert!(validate_overlay_resolution(&resource, "data-managed").is_err());
        for generation in [json!("bad"), json!(4)] {
            let mut resource = healthy();
            resource["metadata"]["generation"] = generation;
            assert!(validate_overlay_resolution(&resource, "data-managed").is_err());
        }
    }
}
