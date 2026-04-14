//! Center aggregation endpoints for RegionRoute PluginMetaData data.
//!
//! GET  /api/v1/center/global-region-pms
//!   Aggregates GlobalRegionRoute PluginMetaData across all online controllers, grouped by (namespace, name).
//!
//! GET  /api/v1/center/service-region-pms
//!   Aggregates ServiceRegionRoute PluginMetaData across all online controllers, grouped by (namespace, name).
//!
//! POST /api/v1/center/global-region-pms/failover
//!   Sets failoverTo for a region in a GlobalRegionRoute PluginMetaData on all online controllers.
//!
//! POST /api/v1/center/service-region-pms/failover
//!   Sets failoverTo for a region in a ServiceRegionRoute PluginMetaData on all online controllers.

use axum::{extract::State, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::region_route_handlers::forward_to_all_online;
use super::ApiState;
use crate::core::common::api::{ApiResponse, ListResponse};

// ---------------------------------------------------------------------------
// DTOs – Controller side
// ---------------------------------------------------------------------------

/// Wrapper matching the controller's JSON envelope `{ success, data }`.
#[derive(Debug, Deserialize)]
struct ControllerListResp<T> {
    success: bool,
    data: Option<T>,
}

/// One entry in either the controller's global or service PluginMetaData list response.
/// Only `namespace` and `name` are used for aggregation; additional fields are ignored.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginMetadataListEntry {
    namespace: String,
    name: String,
}

// ---------------------------------------------------------------------------
// DTOs – Center side (public response types)
// ---------------------------------------------------------------------------

/// Aggregated RegionRoute PluginMetaData entry across all controllers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CenterRegionPluginMetadata {
    namespace: String,
    name: String,
    /// controller_ids that have this (namespace, name).
    controllers: Vec<String>,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

/// Request body for PluginMetaData failover operations.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginMetadataFailoverRequest {
    namespace: String,
    name: String,
    region_name: String,
    failover_to: String,
}

// ---------------------------------------------------------------------------
// Shared aggregation helper
// ---------------------------------------------------------------------------

/// Call every online controller at `controller_path`, aggregate results by
/// `(namespace, name)`, and return a sorted list of `CenterRegionPluginMetadata`.
async fn aggregate_plugin_metadata_list(
    state: &ApiState,
    controller_path: &str,
    endpoint_label: &str,
) -> Vec<CenterRegionPluginMetadata> {
    let responses: Vec<(String, ControllerListResp<Vec<PluginMetadataListEntry>>)> =
        forward_to_all_online(state, controller_path.to_string(), endpoint_label).await;

    let mut map: HashMap<(String, String), Vec<String>> = HashMap::new();

    for (controller_id, parsed) in responses {
        if !parsed.success {
            continue;
        }
        for entry in parsed.data.unwrap_or_default() {
            map.entry((entry.namespace.clone(), entry.name.clone()))
                .or_default()
                .push(controller_id.clone());
        }
    }

    let mut items: Vec<CenterRegionPluginMetadata> = map
        .into_iter()
        .map(|((namespace, name), mut controllers)| {
            controllers.sort();
            CenterRegionPluginMetadata {
                namespace,
                name,
                controllers,
            }
        })
        .collect();

    items.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    items
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/v1/center/global-region-pms`
///
/// Calls every online controller's `GET /api/v1/global-region-pms`, then aggregates
/// results by `(namespace, name)`, collecting which controllers have each entry.
pub async fn list_center_global_region_plugin_metadata(State(state): State<ApiState>) -> impl IntoResponse {
    let items = aggregate_plugin_metadata_list(&state, "/api/v1/global-region-pms", "global-region-pms").await;
    Json(ListResponse::success(items))
}

/// `GET /api/v1/center/service-region-pms`
///
/// Calls every online controller's `GET /api/v1/service-region-pms`, then aggregates
/// results by `(namespace, name)`, collecting which controllers have each entry.
pub async fn list_center_service_region_plugin_metadata(State(state): State<ApiState>) -> impl IntoResponse {
    let items = aggregate_plugin_metadata_list(&state, "/api/v1/service-region-pms", "service-region-pms").await;
    Json(ListResponse::success(items))
}

/// `POST /api/v1/center/global-region-pms/failover`
///
/// Fans out `POST /api/v1/global-region-pms/failover` to all online controllers
/// with the same request body.
///
/// Response: `{ modified: N, failed: N }`
pub async fn global_plugin_metadata_failover(
    State(state): State<ApiState>,
    Json(body): Json<PluginMetadataFailoverRequest>,
) -> impl IntoResponse {
    let body_bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(e) => return Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    };

    let (modified, failed) =
        fan_out_failover(&state, "/api/v1/global-region-pms/failover".to_string(), body_bytes).await;

    Json(ApiResponse::success(serde_json::json!({
        "modified": modified,
        "failed": failed,
    })))
}

/// `POST /api/v1/center/service-region-pms/failover`
///
/// Fans out `POST /api/v1/service-region-pms/failover` to all online controllers
/// with the same request body.
///
/// Response: `{ modified: N, failed: N }`
pub async fn service_plugin_metadata_failover(
    State(state): State<ApiState>,
    Json(body): Json<PluginMetadataFailoverRequest>,
) -> impl IntoResponse {
    let body_bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(e) => return Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    };

    let (modified, failed) =
        fan_out_failover(&state, "/api/v1/service-region-pms/failover".to_string(), body_bytes).await;

    Json(ApiResponse::success(serde_json::json!({
        "modified": modified,
        "failed": failed,
    })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fan-out a POST request to all online controllers in parallel.
/// Returns `(modified_count, failed_count)`.
async fn fan_out_failover(state: &ApiState, path: String, body_bytes: Vec<u8>) -> (usize, usize) {
    let summaries = state.aggregator.controller_summaries();
    let online: Vec<_> = summaries.into_iter().filter(|s| s.online).collect();

    let futs = online.iter().map(|s| {
        let proxy = state.proxy.clone();
        let controller_id = s.controller_id.clone();
        let path = path.clone();
        let body = body_bytes.clone();
        async move {
            let mut headers = HashMap::new();
            headers.insert("content-type".to_string(), "application/json".to_string());
            let result = proxy
                .forward(&controller_id, "POST".to_string(), path, headers, body)
                .await;
            match result {
                Ok(r) if r.status_code == 200 => {
                    tracing::debug!(
                        component = "center",
                        controller_id = %controller_id,
                        "PluginMetaData failover forwarded successfully"
                    );
                    true
                }
                Ok(r) => {
                    tracing::warn!(
                        component = "center",
                        controller_id = %controller_id,
                        status = r.status_code,
                        "PluginMetaData failover POST returned non-OK status"
                    );
                    false
                }
                Err((status, msg)) => {
                    tracing::warn!(
                        component = "center",
                        controller_id = %controller_id,
                        status = %status,
                        error = %msg,
                        "PluginMetaData failover POST failed"
                    );
                    false
                }
            }
        }
    });

    let results = futures::future::join_all(futs).await;
    let modified = results.iter().filter(|&&ok| ok).count();
    let failed = results.iter().filter(|&&ok| !ok).count();
    (modified, failed)
}
