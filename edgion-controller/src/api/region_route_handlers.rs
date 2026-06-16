//! Region Route API handlers
//!
//! POST /api/v1/region-routes/failover
//!   Patches a single region's `failoverTo` in a `RegionRouteOverride` EdgionConfigData.
//!
//! NOTE: The old `/api/v1/cluster-region-routes` and `/api/v1/service-region-routes`
//! list endpoints are removed as part of the S3 RegionRoute decomposition.
//! The routing config (logic + default regions) now lives entirely in EdgionPlugins
//! (`RegionRouteConfig.regions`). Dynamic region overrides are `RegionRouteOverride`
//! EdgionConfigData, which can be listed via the standard EdgionConfigData configserver API.

use axum::{extract::State, http::StatusCode, response::Json};
use serde::Deserialize;
use std::sync::Arc;

use super::types::AdminState;

// ============= Types =============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgionConfigDataFailoverRequest {
    pub namespace: String,
    pub name: String,
    pub region_name: String,
    /// Empty string = clear failover.
    pub failover_to: String,
}

// ============= Handlers =============

/// `GET /api/v1/cluster-region-routes` — deprecated; returns an empty list.
///
/// After S3 the ClusterRegionRoute EdgionConfigData variant is removed. Region topology
/// now lives inline in EdgionPlugins `RegionRouteConfig.regions`. Dynamic overrides
/// are `RegionRouteOverride` EdgionConfigData, listable via the configserver API.
pub async fn list_cluster_region_edgion_config_data(
    State(_state): State<Arc<AdminState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    Ok(Json(serde_json::json!({ "success": true, "data": [] })))
}

/// `GET /api/v1/service-region-routes` — deprecated; returns an empty list.
///
/// After S3 the ServiceRegionRoute EdgionConfigData variant is removed. Per-service failover
/// is now expressed via `RegionRouteOverride.regions[].failoverTo` in an EdgionConfigData.
pub async fn list_service_region_edgion_config_data(
    State(_state): State<Arc<AdminState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    Ok(Json(serde_json::json!({ "success": true, "data": [] })))
}

/// `POST /api/v1/cluster-region-routes/failover`
///
/// Patches a single region's `failoverTo` in a `RegionRouteOverride` EdgionConfigData.
/// The `name` field should refer to a `RegionRouteOverride` EdgionConfigData (not the old
/// ClusterRegionRoute — that variant no longer exists).
pub async fn cluster_edgion_config_data_failover(
    State(state): State<Arc<AdminState>>,
    Json(body): Json<EdgionConfigDataFailoverRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match patch_edgion_config_data_failover(
        &state,
        &body.namespace,
        &body.name,
        "RegionRouteOverride",
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
/// Deprecated. After S3, per-service failover is managed via `RegionRouteOverride` EdgionConfigData.
/// This endpoint now delegates to the same `RegionRouteOverride` patch path.
pub async fn service_edgion_config_data_failover(
    State(state): State<Arc<AdminState>>,
    Json(body): Json<EdgionConfigDataFailoverRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match patch_edgion_config_data_failover(
        &state,
        &body.namespace,
        &body.name,
        "RegionRouteOverride",
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

/// GET the EdgionConfigData from local storage, modify the region's failoverTo, then PUT back.
async fn patch_edgion_config_data_failover(
    state: &AdminState,
    namespace: &str,
    edgion_config_data_name: &str,
    plugin_type: &str,
    region_name: &str,
    failover_to: &str,
) -> Result<bool, String> {
    use edgion_resources::ResourceKind;

    // Serialize the whole get -> patch -> write below against other writers to the
    // same resource so concurrent failover requests and a concurrent generic
    // `update_namespaced` PUT cannot interleave and silently lose an update.
    let _resource_guard = super::resource_locks::lock_resource(
        ResourceKind::EdgionConfigData.as_str(),
        namespace,
        edgion_config_data_name,
    )
    .await;

    let resource = state
        .center_get_resource(ResourceKind::EdgionConfigData, namespace, edgion_config_data_name)
        .await
        .map_err(|e| format!("failed to get EdgionConfigData: {e}"))?
        .ok_or_else(|| format!("EdgionConfigData {}/{} not found", namespace, edgion_config_data_name))?;

    let mut resource = resource;

    let mut modified = false;

    // Try processed format: spec.plugins[].type/config
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

    // Try raw file format: spec.data.config.regions
    if !modified {
        if let Some(data) = resource.pointer_mut("/spec/data") {
            if data.get("type").and_then(|t| t.as_str()) == Some(plugin_type) {
                if let Some(regions) = data.pointer_mut("/config/regions").and_then(|r| r.as_array_mut()) {
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
        ResourceKind::EdgionConfigData.as_str(),
        Some(namespace),
        edgion_config_data_name,
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
            "name": "test-override-route",
            "regionName": "east",
            "failoverTo": "west",
            "myRegion": "evil"
        }"#;
        let result: Result<EdgionConfigDataFailoverRequest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown field 'myRegion' must be rejected");
    }

    #[test]
    fn failover_request_accepts_canonical_payload() {
        let json = r#"{
            "namespace": "default",
            "name": "test-override-route",
            "regionName": "east",
            "failoverTo": "west"
        }"#;
        let result: Result<EdgionConfigDataFailoverRequest, _> = serde_json::from_str(json);
        let req = result.expect("canonical 4-field payload must be accepted");
        assert_eq!(req.namespace, "default");
        assert_eq!(req.name, "test-override-route");
        assert_eq!(req.region_name, "east");
        assert_eq!(req.failover_to, "west");
    }

    #[test]
    fn failover_request_accepts_empty_failover_to() {
        let json = r#"{
            "namespace": "default",
            "name": "test-override-route",
            "regionName": "east",
            "failoverTo": ""
        }"#;
        let result: Result<EdgionConfigDataFailoverRequest, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().failover_to, "");
    }

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

        assert_eq!(regions[0]["failoverTo"], serde_json::Value::String("west".into()));
        assert_eq!(regions[0]["name"], original[0]["name"]);
        assert_eq!(regions[0]["weight"], original[0]["weight"]);
        assert_eq!(regions[0]["endpoints"], original[0]["endpoints"]);
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

        assert!(regions[0].as_object().unwrap().get("failoverTo").is_none());
        assert_eq!(regions[0]["name"], original[0]["name"]);
        assert_eq!(regions[0]["weight"], original[0]["weight"]);
        assert_eq!(regions[1], original[1]);
    }

    #[test]
    fn patch_region_failover_no_match_returns_false() {
        let mut regions = vec![serde_json::json!({"name": "east", "failoverTo": "x"})];
        let modified = patch_region_failover(&mut regions, "nonexistent", "y");
        assert!(!modified);
        assert_eq!(regions[0]["failoverTo"], serde_json::Value::String("x".into()));
    }
}
