//! Region Route PluginMetaData API handlers
//!
//! GET /api/v1/cluster-region-routes
//!   Lists all ClusterRegionRoute entries from the pre-built table.
//!
//! GET /api/v1/service-region-routes
//!   Lists all ServiceRegionRoute entries from the pre-built table.
//!
//! POST /api/v1/cluster-region-routes/failover
//!   Modifies a single region's failoverTo in a ClusterRegionRoute PluginMetaData.
//!
//! POST /api/v1/service-region-routes/failover
//!   Modifies a single region's failoverTo in a ServiceRegionRoute PluginMetaData.

use axum::{extract::State, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use edgion_common::api::ListResponse;

use super::types::AdminState;

// ============= ClusterRegionRoute PluginMetaData Types =============

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRegionPluginMetadataEntry {
    pub namespace: String,
    pub name: String,
    pub my_region: String,
    pub regions: serde_json::Value,
    pub key_get: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_key_get: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_calc: Option<serde_json::Value>,
    pub route_rules: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_by_key_conf_match: Option<serde_json::Value>,
}

// ============= ServiceRegionRoute PluginMetaData Types =============

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRegionPluginMetadataEntry {
    pub namespace: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_ref: Option<serde_json::Value>,
    pub regions: serde_json::Value,
    pub ref_plugins: serde_json::Value,
}

// ============= Failover Types =============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginMetadataFailoverRequest {
    pub namespace: String,
    pub name: String,
    pub region_name: String,
    /// Empty string = clear failover.
    pub failover_to: String,
}

// ============= Handlers =============

/// `GET /api/v1/cluster-region-routes`
///
/// Lists all ClusterRegionRoute PluginMetaData entries from the pre-built table.
pub async fn list_cluster_region_plugin_metadata(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<ListResponse<ClusterRegionPluginMetadataEntry>>, StatusCode> {
    let runtime = state.runtime();
    let entries = runtime.cluster_region_route_table.all();

    let mut result: Vec<ClusterRegionPluginMetadataEntry> = entries
        .into_iter()
        .map(|e| ClusterRegionPluginMetadataEntry {
            namespace: e.pm_namespace,
            name: e.pm_name,
            my_region: e.my_region,
            regions: serde_json::to_value(&e.regions).unwrap_or_default(),
            key_get: serde_json::to_value(&e.key_get).unwrap_or_default(),
            hash_key_get: e.hash_key_get.as_ref().and_then(|v| serde_json::to_value(v).ok()),
            hash_calc: e.hash_calc.as_ref().and_then(|v| serde_json::to_value(v).ok()),
            route_rules: serde_json::to_value(&e.route_rules).unwrap_or_default(),
            route_by_key_conf_match: e
                .route_by_key_conf_match
                .as_ref()
                .and_then(|v| serde_json::to_value(v).ok()),
        })
        .collect();

    result.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    Ok(Json(ListResponse::success(result)))
}

/// `GET /api/v1/service-region-routes`
///
/// Lists all ServiceRegionRoute PluginMetaData entries from the pre-built table.
pub async fn list_service_region_plugin_metadata(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<ListResponse<ServiceRegionPluginMetadataEntry>>, StatusCode> {
    let runtime = state.runtime();
    let entries = runtime.service_region_route_table.all();

    let mut result: Vec<ServiceRegionPluginMetadataEntry> = entries
        .into_iter()
        .map(|e| ServiceRegionPluginMetadataEntry {
            namespace: e.pm_namespace,
            name: e.pm_name,
            cluster_ref: e.cluster_pm_ref.as_ref().and_then(|v| serde_json::to_value(v).ok()),
            regions: serde_json::to_value(&e.regions).unwrap_or_default(),
            ref_plugins: serde_json::to_value(&e.ref_plugins).unwrap_or_default(),
        })
        .collect();

    result.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    Ok(Json(ListResponse::success(result)))
}

/// `POST /api/v1/cluster-region-routes/failover`
///
/// Modifies a single region's failoverTo in a local ClusterRegionRoute PluginMetaData.
pub async fn cluster_plugin_metadata_failover(
    State(state): State<Arc<AdminState>>,
    Json(body): Json<PluginMetadataFailoverRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match patch_plugin_metadata_failover(
        &state,
        &body.namespace,
        &body.name,
        "ClusterRegionRoute",
        &body.region_name,
        &body.failover_to,
    )
    .await
    {
        Ok(true) => Ok(Json(serde_json::json!({ "success": true, "modified": true }))),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(_e) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// `POST /api/v1/service-region-routes/failover`
///
/// Modifies a single region's failoverTo in a local ServiceRegionRoute PluginMetaData.
pub async fn service_plugin_metadata_failover(
    State(state): State<Arc<AdminState>>,
    Json(body): Json<PluginMetadataFailoverRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match patch_plugin_metadata_failover(
        &state,
        &body.namespace,
        &body.name,
        "ServiceRegionRoute",
        &body.region_name,
        &body.failover_to,
    )
    .await
    {
        Ok(true) => Ok(Json(serde_json::json!({ "success": true, "modified": true }))),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(_e) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// ============= Helpers =============

/// GET the PluginMetaData from local storage, modify the region's failoverTo, then PUT back.
async fn patch_plugin_metadata_failover(
    state: &AdminState,
    namespace: &str,
    plugin_metadata_name: &str,
    plugin_type: &str,
    region_name: &str,
    failover_to: &str,
) -> Result<bool, String> {
    use edgion_resources::ResourceKind;

    // Serialize the whole get -> patch -> write below against other writers to the
    // same resource so concurrent failover requests, and a concurrent generic
    // `update_namespaced` PUT (which the Center region-route sync routes through),
    // cannot interleave and silently lose an update. Held until the guard drops at
    // the end of this function. See `super::resource_locks` and
    // `skills/04-review/concurrency/failover-read-modify-write-per-key-lock.md`.
    let _resource_guard =
        super::resource_locks::lock_resource(ResourceKind::PluginMetaData.as_str(), namespace, plugin_metadata_name)
            .await;

    let resource = state
        .center_get_resource(ResourceKind::PluginMetaData, namespace, plugin_metadata_name)
        .await
        .map_err(|e| format!("failed to get PluginMetaData: {e}"))?
        .ok_or_else(|| format!("PluginMetaData {}/{} not found", namespace, plugin_metadata_name))?;

    let mut resource = resource;

    // Find the plugin entry and modify the region's failoverTo.
    // Support both storage formats:
    //   - spec.plugins[].type/config (processed format)
    //   - spec.metadata.type/config  (raw file format)
    let mut modified = false;

    // Try processed format: spec.plugins[]
    if let Some(plugins) = resource.pointer_mut("/spec/plugins").and_then(|p| p.as_array_mut()) {
        for plugin in plugins.iter_mut() {
            if plugin.get("type").and_then(|t| t.as_str()) != Some(plugin_type) {
                continue;
            }
            if let Some(regions) = plugin.pointer_mut("/config/regions").and_then(|r| r.as_array_mut()) {
                modified = patch_region_failover(regions, region_name, failover_to);
            }
        }
    }

    // Try raw file format: spec.metadata
    if !modified {
        if let Some(metadata) = resource.pointer_mut("/spec/metadata") {
            if metadata.get("type").and_then(|t| t.as_str()) == Some(plugin_type) {
                if let Some(regions) = metadata.pointer_mut("/config/regions").and_then(|r| r.as_array_mut()) {
                    modified = patch_region_failover(regions, region_name, failover_to);
                }
            }
        }
    }

    if !modified {
        return Ok(false);
    }

    let json_content = serde_json::to_string(&resource).map_err(|e| format!("JSON serialize failed: {}", e))?;

    let api = state.conf_center();
    api.update_one(
        ResourceKind::PluginMetaData.as_str(),
        Some(namespace),
        plugin_metadata_name,
        json_content,
    )
    .await
    .map_err(|e| format!("update_one failed: {}", e))?;

    Ok(true)
}

/// Modify a single region's failoverTo in a regions array. Returns true if modified.
fn patch_region_failover(regions: &mut Vec<serde_json::Value>, region_name: &str, failover_to: &str) -> bool {
    let mut modified = false;
    for region in regions.iter_mut() {
        if region.get("name").and_then(|n| n.as_str()) != Some(region_name) {
            continue;
        }
        if failover_to.is_empty() {
            if let Some(obj) = region.as_object_mut() {
                obj.remove("failoverTo");
            }
        } else {
            region["failoverTo"] = serde_json::Value::String(failover_to.to_string());
        }
        modified = true;
    }
    modified
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failover_request_rejects_unknown_fields() {
        // Any unknown field must be rejected to keep the contract
        // "failover endpoint can only change failoverTo".
        let json = r#"{
            "namespace": "default",
            "name": "test-cluster-route",
            "regionName": "east",
            "failoverTo": "west",
            "myRegion": "evil"
        }"#;
        let result: Result<PluginMetadataFailoverRequest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown field 'myRegion' must be rejected");
    }

    #[test]
    fn failover_request_accepts_canonical_payload() {
        let json = r#"{
            "namespace": "default",
            "name": "test-cluster-route",
            "regionName": "east",
            "failoverTo": "west"
        }"#;
        let result: Result<PluginMetadataFailoverRequest, _> = serde_json::from_str(json);
        let req = result.expect("canonical 4-field payload must be accepted");
        // Assert field values to verify rename_all = "camelCase" maps correctly.
        assert_eq!(req.namespace, "default");
        assert_eq!(req.name, "test-cluster-route");
        assert_eq!(req.region_name, "east");
        assert_eq!(req.failover_to, "west");
    }

    #[test]
    fn failover_request_accepts_empty_failover_to() {
        // Empty string semantics = "clear failover".
        let json = r#"{
            "namespace": "default",
            "name": "test-cluster-route",
            "regionName": "east",
            "failoverTo": ""
        }"#;
        let result: Result<PluginMetadataFailoverRequest, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().failover_to, "");
    }

    /// Locked invariant: `patch_region_failover` mutates ONLY the targeted region's
    /// `failoverTo` field. Every other byte of the surrounding region object and other
    /// regions stays identical. This is the runtime guarantee behind the contract
    /// "the failover endpoint cannot modify anything other than failoverTo".
    #[test]
    fn patch_region_failover_only_changes_target_failover_to() {
        let original = serde_json::json!([
            {
                "name": "east",
                "weight": 100,
                "endpoints": ["10.0.0.1", "10.0.0.2"],
                "failoverTo": "old-target"
            },
            {
                "name": "west",
                "weight": 50,
                "endpoints": ["10.0.1.1"],
                "failoverTo": "another"
            }
        ]);
        let mut regions = original.as_array().unwrap().clone();

        let modified = patch_region_failover(&mut regions, "east", "west");
        assert!(modified, "should report modified");

        // Target region: only failoverTo changed
        assert_eq!(regions[0]["failoverTo"], serde_json::Value::String("west".into()));
        assert_eq!(regions[0]["name"], original[0]["name"]);
        assert_eq!(regions[0]["weight"], original[0]["weight"]);
        assert_eq!(regions[0]["endpoints"], original[0]["endpoints"]);

        // Non-target region: byte-for-byte identical
        assert_eq!(regions[1], original[1]);
    }

    #[test]
    fn patch_region_failover_clear_removes_field_only() {
        let original = serde_json::json!([
            {
                "name": "east",
                "weight": 100,
                "failoverTo": "west"
            },
            {
                "name": "west",
                "weight": 50,
                "failoverTo": "east"
            }
        ]);
        let mut regions = original.as_array().unwrap().clone();

        let modified = patch_region_failover(&mut regions, "east", "");
        assert!(modified);

        // Target region: failoverTo removed, other fields untouched
        assert!(regions[0].as_object().unwrap().get("failoverTo").is_none());
        assert_eq!(regions[0]["name"], original[0]["name"]);
        assert_eq!(regions[0]["weight"], original[0]["weight"]);

        // Non-target region: identical
        assert_eq!(regions[1], original[1]);
    }

    #[test]
    fn patch_region_failover_no_match_returns_false() {
        let mut regions = vec![serde_json::json!({"name": "east", "failoverTo": "x"})];
        let modified = patch_region_failover(&mut regions, "nonexistent", "y");
        assert!(!modified);
        // Untouched
        assert_eq!(regions[0]["failoverTo"], serde_json::Value::String("x".into()));
    }
}
