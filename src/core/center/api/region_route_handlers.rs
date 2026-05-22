//! Region Route handlers and shared helpers for Center aggregation handlers.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ApiState;

// ============= Failover Request Type =============

/// Strongly-typed body for `/api/v1/center/{cluster,service}-region-routes/failover`.
///
/// The failover endpoint can only change the targeted region's `failoverTo` field.
/// Any extra field (e.g. `myRegion`, `spec`, full PM YAML) is rejected at the
/// deserialization stage, so the contract is enforced by the type system rather
/// than by runtime checks.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FailoverRequest {
    pub namespace: String,
    pub name: String,
    pub region_name: String,
    /// Empty string = clear failover.
    pub failover_to: String,
}

// ============= RegionRoute MetaDataStore Handlers =============

pub async fn list_cluster_region_routes(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let routes = state.metadata_store.list_cluster_routes();
    Json(serde_json::json!({
        "success": true,
        "data": routes,
    }))
}

pub async fn list_service_region_routes(State(state): State<ApiState>) -> Json<serde_json::Value> {
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
    Json(req): Json<FailoverRequest>,
) -> Json<serde_json::Value> {
    let (modified, failed) = fan_out_failover(&state, "/api/v1/cluster-region-routes/failover".to_string(), &req).await;

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
    Json(req): Json<FailoverRequest>,
) -> Json<serde_json::Value> {
    let (modified, failed) = fan_out_failover(&state, "/api/v1/service-region-routes/failover".to_string(), &req).await;

    Json(serde_json::json!({
        "success": true,
        "data": { "modified": modified, "failed": failed },
    }))
}

// ============= Sync Handlers =============

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub source_controller_id: String,
    pub namespace: String,
    pub name: String,
}

/// `POST /api/v1/center/cluster-region-routes/sync`
///
/// Reads the ClusterRegionRoute PluginMetaData from the source controller,
/// then writes it to all other online controllers. Preserves each target's
/// `myRegion` value (since it's intentionally different per controller).
pub async fn cluster_region_route_sync(
    State(state): State<ApiState>,
    Json(body): Json<SyncRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // 1. Read source PM via proxy
    let source_path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", body.namespace, body.name);
    let source_pm = match state
        .proxy
        .forward(
            &body.source_controller_id,
            "GET".to_string(),
            source_path.clone(),
            HashMap::new(),
            vec![],
        )
        .await
    {
        Ok(r) if r.status_code == 200 => match serde_json::from_slice::<serde_json::Value>(&r.body) {
            // Controller's single-resource GET returns the bare PluginMetaData object,
            // not a {success,data} wrapper. Tolerate both shapes for forward compatibility.
            Ok(v) => v.get("data").cloned().unwrap_or(v),
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "success": false, "error": format!("Parse source PM failed: {e}") })),
                )
            }
        },
        Ok(r) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(
                    serde_json::json!({ "success": false, "error": format!("Source GET returned {}", r.status_code) }),
                ),
            )
        }
        Err((status, msg)) => {
            return (
                status,
                Json(serde_json::json!({ "success": false, "error": format!("Source GET failed: {status} {msg}") })),
            )
        }
    };

    // 2. Get each controller's myRegion from MetaDataStore
    let routes = state.metadata_store.list_cluster_routes();
    let route = routes
        .iter()
        .find(|r| r.namespace == body.namespace && r.name == body.name);
    let my_region_map: HashMap<String, String> = match route {
        Some(r) => r
            .controllers
            .iter()
            .map(|(cid, entry)| (cid.clone(), entry.my_region.clone()))
            .collect(),
        None => HashMap::new(),
    };

    // 3. Fan-out PUT to all online controllers (except source)
    let summaries = state.aggregator.controller_summaries();
    let targets: Vec<_> = summaries
        .into_iter()
        .filter(|s| s.online && s.controller_id != body.source_controller_id)
        .collect();

    let put_path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", body.namespace, body.name);

    let futs = targets.iter().map(|s| {
        let proxy = state.proxy.clone();
        let controller_id = s.controller_id.clone();
        let put_path = put_path.clone();
        let mut pm = source_pm.clone();

        // Preserve target's myRegion
        if let Some(target_my_region) = my_region_map.get(&controller_id) {
            // Try spec.metadata.config.myRegion (raw file format)
            if let Some(mr) = pm.pointer_mut("/spec/metadata/config/myRegion") {
                *mr = serde_json::Value::String(target_my_region.clone());
            }
            // Try spec.plugins[].config.baseInfo.myRegion (processed format)
            if let Some(plugins) = pm.pointer_mut("/spec/plugins").and_then(|p| p.as_array_mut()) {
                for plugin in plugins.iter_mut() {
                    if let Some(mr) = plugin.pointer_mut("/config/baseInfo/myRegion") {
                        *mr = serde_json::Value::String(target_my_region.clone());
                    }
                }
            }
        }

        async move {
            let content = match serde_json::to_vec(&pm) {
                Ok(b) => b,
                Err(_) => return false,
            };
            let mut headers = HashMap::new();
            headers.insert("content-type".to_string(), "application/json".to_string());
            match proxy
                .forward(&controller_id, "PUT".to_string(), put_path, headers, content)
                .await
            {
                Ok(r) if r.status_code == 200 => true,
                _ => {
                    tracing::warn!(component = "center", controller_id = %controller_id, "Sync PUT failed");
                    false
                }
            }
        }
    });

    let results = futures::future::join_all(futs).await;
    let modified = results.iter().filter(|&&ok| ok).count() + 1; // +1 for source
    let failed = results.iter().filter(|&&ok| !ok).count();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "success": true,
            "data": { "modified": modified, "failed": failed },
        })),
    )
}

/// `POST /api/v1/center/service-region-routes/sync`
///
/// Same as cluster sync but for ServiceRegionRoute. No myRegion to preserve.
pub async fn service_region_route_sync(
    State(state): State<ApiState>,
    Json(body): Json<SyncRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // 1. Read source PM via proxy
    let source_path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", body.namespace, body.name);
    let source_pm = match state
        .proxy
        .forward(
            &body.source_controller_id,
            "GET".to_string(),
            source_path,
            HashMap::new(),
            vec![],
        )
        .await
    {
        Ok(r) if r.status_code == 200 => match serde_json::from_slice::<serde_json::Value>(&r.body) {
            // Controller's single-resource GET returns the bare PluginMetaData object,
            // not a {success,data} wrapper. Tolerate both shapes for forward compatibility.
            Ok(v) => v.get("data").cloned().unwrap_or(v),
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "success": false, "error": format!("Parse source PM failed: {e}") })),
                )
            }
        },
        Ok(r) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(
                    serde_json::json!({ "success": false, "error": format!("Source GET returned {}", r.status_code) }),
                ),
            )
        }
        Err((status, msg)) => {
            return (
                status,
                Json(serde_json::json!({ "success": false, "error": format!("Source GET failed: {status} {msg}") })),
            )
        }
    };

    // 2. Fan-out PUT to all online controllers (except source)
    let summaries = state.aggregator.controller_summaries();
    let targets: Vec<_> = summaries
        .into_iter()
        .filter(|s| s.online && s.controller_id != body.source_controller_id)
        .collect();

    let put_path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", body.namespace, body.name);

    let futs = targets.iter().map(|s| {
        let proxy = state.proxy.clone();
        let controller_id = s.controller_id.clone();
        let put_path = put_path.clone();
        let pm = source_pm.clone();

        async move {
            let content = match serde_json::to_vec(&pm) {
                Ok(b) => b,
                Err(_) => return false,
            };
            let mut headers = HashMap::new();
            headers.insert("content-type".to_string(), "application/json".to_string());
            match proxy
                .forward(&controller_id, "PUT".to_string(), put_path, headers, content)
                .await
            {
                Ok(r) if r.status_code == 200 => true,
                _ => {
                    tracing::warn!(component = "center", controller_id = %controller_id, "Service sync PUT failed");
                    false
                }
            }
        }
    });

    let results = futures::future::join_all(futs).await;
    let modified = results.iter().filter(|&&ok| ok).count() + 1;
    let failed = results.iter().filter(|&&ok| !ok).count();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "success": true,
            "data": { "modified": modified, "failed": failed },
        })),
    )
}

/// Fan-out a POST request to all online controllers in parallel.
/// Returns `(modified_count, failed_count)`.
///
/// Body is re-serialized from the typed `FailoverRequest` here — Center never
/// forwards raw upstream bytes, ensuring controllers always receive a clean
/// 4-field payload regardless of how the operator structured the original request.
async fn fan_out_failover(state: &ApiState, path: String, req: &FailoverRequest) -> (usize, usize) {
    let body_bytes = match serde_json::to_vec(req) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(
                component = "center",
                error = %e,
                "FailoverRequest serialization failed unexpectedly"
            );
            unreachable!("FailoverRequest contains only String fields; serialization cannot fail");
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failover_request_rejects_unknown_fields() {
        let json = r#"{
            "namespace": "default",
            "name": "test-cluster-route",
            "regionName": "east",
            "failoverTo": "west",
            "myRegion": "evil"
        }"#;
        let result: Result<FailoverRequest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown field 'myRegion' must be rejected at center");
    }

    #[test]
    fn failover_request_rejects_full_pm_payload() {
        // A real-world risk: someone tries to send a full PluginMetaData YAML/JSON.
        let json = r#"{
            "namespace": "default",
            "name": "test-cluster-route",
            "regionName": "east",
            "failoverTo": "west",
            "spec": {"metadata": {"config": {"myRegion": "evil"}}}
        }"#;
        let result: Result<FailoverRequest, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "spec field must be rejected — center failover does not accept full PM"
        );
    }

    #[test]
    fn failover_request_roundtrip_canonical() {
        let json = r#"{"namespace":"default","name":"r","regionName":"east","failoverTo":"west"}"#;
        let req: FailoverRequest = serde_json::from_str(json).expect("canonical payload accepted");
        assert_eq!(req.namespace, "default");
        assert_eq!(req.region_name, "east");
        assert_eq!(req.failover_to, "west");
        // Re-serialize: only the 4 contract fields, nothing else.
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"regionName\":\"east\""));
        assert!(!s.contains("myRegion"));
        assert!(!s.contains("spec"));
    }
}
