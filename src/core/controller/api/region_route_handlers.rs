//! Region Route PluginMetaData API handlers
//!
//! GET /api/v1/global-region-pms
//!   Lists all GlobalRegionRoute entries from the pre-built table.
//!
//! GET /api/v1/service-region-pms
//!   Lists all ServiceRegionRoute entries from the pre-built table.
//!
//! POST /api/v1/global-region-pms/failover
//!   Modifies a single region's failoverTo in a GlobalRegionRoute PluginMetaData.
//!
//! POST /api/v1/service-region-pms/failover
//!   Modifies a single region's failoverTo in a ServiceRegionRoute PluginMetaData.

use axum::{extract::State, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::common::api::ListResponse;

use super::types::AdminState;

// ============= GlobalRegionRoute PluginMetaData Types =============

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalRegionPluginMetadataEntry {
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
    pub global_ref: Option<serde_json::Value>,
    pub regions: serde_json::Value,
    pub ref_plugins: serde_json::Value,
}

// ============= Failover Types =============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMetadataFailoverRequest {
    pub namespace: String,
    pub name: String,
    pub region_name: String,
    /// Empty string = clear failover.
    pub failover_to: String,
}

// ============= Handlers =============

/// `GET /api/v1/global-region-pms`
///
/// Lists all GlobalRegionRoute PluginMetaData entries from the pre-built table.
pub async fn list_global_region_plugin_metadata(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<ListResponse<GlobalRegionPluginMetadataEntry>>, StatusCode> {
    let runtime = state.runtime();
    let entries = runtime.global_region_route_table.all();

    let mut result: Vec<GlobalRegionPluginMetadataEntry> = entries
        .into_iter()
        .map(|e| GlobalRegionPluginMetadataEntry {
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

/// `GET /api/v1/service-region-pms`
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
            global_ref: e.global_pm_ref.as_ref().and_then(|v| serde_json::to_value(v).ok()),
            regions: serde_json::to_value(&e.regions).unwrap_or_default(),
            ref_plugins: serde_json::to_value(&e.ref_plugins).unwrap_or_default(),
        })
        .collect();

    result.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    Ok(Json(ListResponse::success(result)))
}

/// `POST /api/v1/global-region-pms/failover`
///
/// Modifies a single region's failoverTo in a local GlobalRegionRoute PluginMetaData.
pub async fn global_plugin_metadata_failover(
    State(state): State<Arc<AdminState>>,
    Json(body): Json<PluginMetadataFailoverRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match patch_plugin_metadata_failover(
        &state,
        &body.namespace,
        &body.name,
        "GlobalRegionRoute",
        &body.region_name,
        &body.failover_to,
    )
    .await
    {
        Ok(true) => Ok(Json(serde_json::json!({ "success": true, "modified": true }))),
        Ok(false) => Ok(Json(
            serde_json::json!({ "success": false, "error": "no matching region found" }),
        )),
        Err(e) => Ok(Json(serde_json::json!({ "success": false, "error": e }))),
    }
}

/// `POST /api/v1/service-region-pms/failover`
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
        Ok(false) => Ok(Json(
            serde_json::json!({ "success": false, "error": "no matching region found" }),
        )),
        Err(e) => Ok(Json(serde_json::json!({ "success": false, "error": e }))),
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
    use crate::types::ResourceKind;

    let resource = state
        .center_get_resource(ResourceKind::PluginMetaData, namespace, plugin_metadata_name)
        .await
        .map_err(|e| format!("failed to get PluginMetaData: {e}"))?
        .ok_or_else(|| format!("PluginMetaData {}/{} not found", namespace, plugin_metadata_name))?;

    let mut resource = resource;

    // Find the plugin entry and modify the region's failoverTo
    let mut modified = false;
    if let Some(plugins) = resource.pointer_mut("/spec/plugins").and_then(|p| p.as_array_mut()) {
        for plugin in plugins.iter_mut() {
            if plugin.get("type").and_then(|t| t.as_str()) != Some(plugin_type) {
                continue;
            }
            if let Some(regions) = plugin.pointer_mut("/config/regions").and_then(|r| r.as_array_mut()) {
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
