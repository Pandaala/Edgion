//! Region Route handlers and shared helpers for Center aggregation handlers.

use axum::extract::State;
use axum::Json;
use std::collections::HashMap;

use super::ApiState;

// ============= RegionRoute MetaDataStore Handlers =============

pub async fn list_cluster_region_routes(
    State(state): State<ApiState>,
) -> Json<serde_json::Value> {
    let routes = state.metadata_store.list_cluster_routes();
    Json(serde_json::json!({
        "success": true,
        "data": routes,
    }))
}

pub async fn list_service_region_routes(
    State(state): State<ApiState>,
) -> Json<serde_json::Value> {
    let routes = state.metadata_store.list_service_routes();
    Json(serde_json::json!({
        "success": true,
        "data": routes,
    }))
}

// ============= Failover Handlers =============

/// `POST /api/v1/center/cluster-region-routes/failover`
///
/// Fans out `POST /api/v1/cluster-region-routes/failover` to all online controllers
/// with the same request body.
///
/// Response: `{ modified: N, failed: N }`
pub async fn cluster_region_route_failover(
    State(state): State<ApiState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let body_bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(e) => return Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    };

    let (modified, failed) =
        fan_out_failover(&state, "/api/v1/cluster-region-routes/failover".to_string(), body_bytes).await;

    Json(serde_json::json!({
        "success": true,
        "data": { "modified": modified, "failed": failed },
    }))
}

/// `POST /api/v1/center/service-region-routes/failover`
///
/// Fans out `POST /api/v1/service-region-routes/failover` to all online controllers
/// with the same request body.
///
/// Response: `{ modified: N, failed: N }`
pub async fn service_region_route_failover(
    State(state): State<ApiState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let body_bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(e) => return Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    };

    let (modified, failed) =
        fan_out_failover(&state, "/api/v1/service-region-routes/failover".to_string(), body_bytes).await;

    Json(serde_json::json!({
        "success": true,
        "data": { "modified": modified, "failed": failed },
    }))
}

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
                        "RegionRoute failover forwarded successfully"
                    );
                    true
                }
                Ok(r) => {
                    tracing::warn!(
                        component = "center",
                        controller_id = %controller_id,
                        status = r.status_code,
                        "RegionRoute failover POST returned non-OK status"
                    );
                    false
                }
                Err((status, msg)) => {
                    tracing::warn!(
                        component = "center",
                        controller_id = %controller_id,
                        status = %status,
                        error = %msg,
                        "RegionRoute failover POST failed"
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

