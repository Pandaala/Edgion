//! HTTP handlers for /api/v1/center/global-connection-ip-restrictions endpoints.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::core::center::api::ApiState;
use crate::core::center::metadata_store::ControllerPmEntry;
use crate::core::common::api::ApiResponse;
use crate::core::common::observe::fed_metrics;

/// Classify a fan-out outcome into the `result` label value emitted on
/// `edgion_fed_fanout_total{op,result}`. "ok" means every target
/// succeeded, "fail" means every target failed, "partial" is the mixed
/// case. Zero-target calls are treated as "ok" since there is nothing
/// that could have failed.
fn fanout_result_label(success_count: usize, failed_count: usize) -> &'static str {
    if failed_count == 0 {
        fed_metrics::labels::fanout_result::OK
    } else if success_count == 0 {
        fed_metrics::labels::fanout_result::FAIL
    } else {
        fed_metrics::labels::fanout_result::PARTIAL
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CenterGlobalIpRestrictionView {
    pub namespace: String,
    pub name: String,
    pub controllers: HashMap<String, ControllerPmEntry>,
    pub online_controller_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsistencyResult {
    pub namespace: String,
    pub name: String,
    pub consistent: bool,
    pub controller_count: usize,
    pub conflicts: Vec<String>,
}

fn online_controllers(state: &ApiState) -> Vec<String> {
    state
        .aggregator
        .controller_summaries()
        .into_iter()
        .filter(|s| s.online)
        .map(|s| s.controller_id)
        .collect()
}

/// `GET /api/v1/center/global-connection-ip-restrictions`
pub async fn list_global_ip_restrictions(
    State(state): State<ApiState>,
) -> Json<ApiResponse<Vec<CenterGlobalIpRestrictionView>>> {
    let entries = state.metadata_store.list_global_ip_restrictions();
    let online = online_controllers(&state);
    let items: Vec<_> = entries
        .into_iter()
        .map(|e| CenterGlobalIpRestrictionView {
            namespace: e.namespace,
            name: e.name,
            controllers: e.controllers,
            online_controller_ids: online.clone(),
        })
        .collect();
    Json(ApiResponse::ok_body(items))
}

/// `GET /api/v1/center/global-connection-ip-restrictions/{ns}/{name}`
pub async fn get_global_ip_restriction(
    State(state): State<ApiState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<ApiResponse<CenterGlobalIpRestrictionView>>, (StatusCode, Json<ApiResponse<()>>)> {
    let pm_key = format!("{}/{}", ns, name);
    let Some(per_ctrl) = state.metadata_store.get_global_ip_restriction(&pm_key) else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_body(format!(
                "GlobalConnectionIpRestriction {}/{} not found",
                ns, name
            ))),
        ));
    };
    let online = online_controllers(&state);
    Ok(Json(ApiResponse::ok_body(CenterGlobalIpRestrictionView {
        namespace: ns,
        name,
        controllers: per_ctrl,
        online_controller_ids: online,
    })))
}

/// `GET /api/v1/center/global-connection-ip-restrictions/consistency`
pub async fn global_ip_restrictions_consistency(
    State(state): State<ApiState>,
) -> Json<ApiResponse<Vec<ConsistencyResult>>> {
    let entries = state.metadata_store.list_global_ip_restrictions();
    let items: Vec<_> = entries
        .into_iter()
        .map(|e| {
            let hashes: HashSet<&str> = e.controllers.values().map(|c| c.content_hash.as_str()).collect();
            let consistent = hashes.len() <= 1;
            // v1: report whole-entry mismatch; per-field conflicts left to v2
            let conflicts = if consistent {
                Vec::new()
            } else {
                vec!["contentHash".to_string()]
            };
            ConsistencyResult {
                namespace: e.namespace,
                name: e.name,
                consistent,
                controller_count: e.controllers.len(),
                conflicts,
            }
        })
        .collect();
    Json(ApiResponse::ok_body(items))
}

// =====================================================================
// Write endpoints: fan-out helpers and request/response types
// =====================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerOpResult {
    pub controller_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanOutResponse {
    pub success: Vec<ControllerOpResult>,
    pub failed: Vec<ControllerOpResult>,
    pub warnings: Vec<String>,
}

/// Resolve `controllers: ["all"]` or `["ctrl-a","ctrl-b"]` into (targets, warnings).
/// `["all"]` → all online controllers. Offline explicit IDs stay in targets (the write will fail them).
fn resolve_targets(state: &ApiState, requested: &[String]) -> (Vec<String>, Vec<String>) {
    if requested.len() == 1 && requested[0] == "all" {
        let online = online_controllers(state);
        let warnings: Vec<String> = state
            .aggregator
            .controller_summaries()
            .into_iter()
            .filter(|s| !s.online)
            .map(|s| format!("{} offline, skipped", s.controller_id))
            .collect();
        (online, warnings)
    } else {
        (requested.to_vec(), Vec::new())
    }
}

/// Fan-out a HTTP op to N controllers in parallel, collecting per-controller outcomes.
///
/// Note: `proxy.forward` returns `Result<HttpProxyResponse, (StatusCode, String)>`.
/// `HttpProxyResponse.status_code` is `u32` (from proto uint32).
async fn fan_out_http(
    state: &ApiState,
    controllers: Vec<String>,
    method: &str,
    path: String,
    body: Vec<u8>,
) -> Vec<ControllerOpResult> {
    let futs = controllers.into_iter().map(|cid| {
        let proxy = state.proxy.clone();
        let method = method.to_string();
        let path = path.clone();
        let body = body.clone();
        async move {
            let mut headers = HashMap::new();
            headers.insert("content-type".to_string(), "application/json".to_string());
            match proxy.forward(&cid, method, path, headers, body).await {
                Ok(r) if r.status_code >= 200 && r.status_code < 300 => ControllerOpResult {
                    controller_id: cid,
                    detail: Some(format!("status {}", r.status_code)),
                    error: None,
                    status_code: Some(r.status_code as u16),
                },
                Ok(r) => ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some(format!("HTTP {}", r.status_code)),
                    status_code: Some(r.status_code as u16),
                },
                Err((status, msg)) => ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some(msg),
                    status_code: Some(status.as_u16()),
                },
            }
        }
    });
    futures::future::join_all(futs).await
}

fn partition(results: Vec<ControllerOpResult>) -> (Vec<ControllerOpResult>, Vec<ControllerOpResult>) {
    let (success, failed): (Vec<_>, Vec<_>) = results.into_iter().partition(|r| r.error.is_none());
    (success, failed)
}

/// Map a fan-out outcome to an HTTP status. When every target failed (success
/// empty, failed non-empty) we surface `502 Bad Gateway` so automated callers
/// can gate retries on the status line without parsing the nested body. Any
/// partial success — and the zero-target case — stays `200 OK`.
fn fanout_status(success: &[ControllerOpResult], failed: &[ControllerOpResult]) -> StatusCode {
    if success.is_empty() && !failed.is_empty() {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::OK
    }
}

// =====================================================================
// POST /api/v1/center/global-connection-ip-restrictions
// =====================================================================

use edgion_resources::resources::plugin_metadata::GlobalConnectionIpRestrictionData;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    pub namespace: String,
    pub name: String,
    pub controllers: Vec<String>,
    pub data: GlobalConnectionIpRestrictionData,
}

/// `POST /api/v1/center/global-connection-ip-restrictions`
pub async fn create_global_ip_restriction(
    State(state): State<ApiState>,
    Json(req): Json<CreateRequest>,
) -> (StatusCode, Json<ApiResponse<FanOutResponse>>) {
    // Early validation: bad request body must surface as HTTP 400 so automated
    // callers can rely on the status code instead of inspecting nested fields.
    {
        let mut cloned = req.data.clone();
        if let Err(e) = cloned.validate_and_init() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::err_body(format!("invalid PM data: {}", e))),
            );
        }
    }

    let (targets, warnings) = resolve_targets(&state, &req.controllers);

    let pm_json = build_pm_json(&req.namespace, &req.name, &req.data);
    let body = pm_json.into_bytes();
    // POST /api/v1/namespaced/pluginmetadata/{namespace} — create on Controller.
    // Name is embedded in the JSON body; POST path does not include the name segment.
    let path = format!("/api/v1/namespaced/pluginmetadata/{}", req.namespace);
    let results = fan_out_http(&state, targets, "POST", path, body).await;
    let (success, failed) = partition(results);
    let status = fanout_status(&success, &failed);
    (
        status,
        Json(ApiResponse::ok_body(FanOutResponse {
            success,
            failed,
            warnings,
        })),
    )
}

/// Extract [`GlobalConnectionIpRestrictionData`] from a Controller GET response body.
///
/// The Controller stores and returns PluginMetaData as:
/// `{"spec": {"metadata": {"config": <GlobalConnectionIpRestrictionData>}}}`
fn extract_pm_data(v: &serde_json::Value) -> Result<GlobalConnectionIpRestrictionData, String> {
    let config = v
        .pointer("/spec/metadata/config")
        .ok_or_else(|| "missing /spec/metadata/config in Controller PM response".to_string())?;
    serde_json::from_value(config.clone()).map_err(|e| format!("failed to deserialize PM config: {e}"))
}

/// Fetch live PM data from a specific Controller via GET.
///
/// Returns `Err((status_code, message))` on any failure so the caller can build a
/// `ControllerOpResult` with the correct `controller_id` and forward the reason to the client.
async fn fetch_live_pm_data(
    proxy: &crate::core::center::proxy::ProxyForwarder,
    cid: &str,
    path: &str,
) -> Result<GlobalConnectionIpRestrictionData, (u16, String)> {
    match proxy
        .forward(cid, "GET".to_string(), path.to_string(), HashMap::new(), vec![])
        .await
    {
        Ok(r) if r.status_code == 200 => {
            let v = serde_json::from_slice::<serde_json::Value>(&r.body)
                .map_err(|e| (502u16, format!("Controller returned non-JSON body: {e}")))?;
            extract_pm_data(&v).map_err(|e| (502u16, format!("GET PM from controller: {e}")))
        }
        Ok(r) if r.status_code == 404 => Err((404, "PM not found on this controller".to_string())),
        Ok(r) => Err((
            r.status_code as u16,
            format!("GET PM from controller returned HTTP {}", r.status_code),
        )),
        Err((_, msg)) => Err((502, format!("GET PM from controller failed: {msg}"))),
    }
}

fn build_pm_json(ns: &str, name: &str, data: &GlobalConnectionIpRestrictionData) -> String {
    let doc = serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "PluginMetaData",
        "metadata": { "name": name, "namespace": ns },
        "spec": {
            "metadata": {
                "type": "GlobalConnectionIpRestriction",
                "config": data
            }
        }
    });
    serde_json::to_string(&doc).expect("serde_json serialize")
}

// =====================================================================
// PUT /api/v1/center/global-connection-ip-restrictions/{ns}/{name}
// =====================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRequest {
    pub controllers: Vec<String>,
    pub data: GlobalConnectionIpRestrictionData,
}

/// `PUT /api/v1/center/global-connection-ip-restrictions/{ns}/{name}`
pub async fn update_global_ip_restriction(
    State(state): State<ApiState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<UpdateRequest>,
) -> (StatusCode, Json<ApiResponse<FanOutResponse>>) {
    // Early validation: bad request body must surface as HTTP 400 so automated
    // callers can rely on the status code instead of inspecting nested fields.
    {
        let mut cloned = req.data.clone();
        if let Err(e) = cloned.validate_and_init() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::err_body(format!("invalid PM data: {}", e))),
            );
        }
    }
    let (targets, warnings) = resolve_targets(&state, &req.controllers);
    let body = build_pm_json(&ns, &name, &req.data).into_bytes();
    let path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", ns, name);
    let results = fan_out_http(&state, targets, "PUT", path, body).await;
    let (success, failed) = partition(results);
    let status = fanout_status(&success, &failed);
    (
        status,
        Json(ApiResponse::ok_body(FanOutResponse {
            success,
            failed,
            warnings,
        })),
    )
}

// =====================================================================
// DELETE /api/v1/center/global-connection-ip-restrictions/{ns}/{name}
// =====================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRequest {
    pub controllers: Vec<String>,
}

/// `DELETE /api/v1/center/global-connection-ip-restrictions/{ns}/{name}`
pub async fn delete_global_ip_restriction(
    State(state): State<ApiState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<DeleteRequest>,
) -> (StatusCode, Json<ApiResponse<FanOutResponse>>) {
    let (targets, warnings) = resolve_targets(&state, &req.controllers);
    let path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", ns, name);
    let results = fan_out_http(&state, targets, "DELETE", path, vec![]).await;
    let (success, failed) = partition(results);
    let status = fanout_status(&success, &failed);
    (
        status,
        Json(ApiResponse::ok_body(FanOutResponse {
            success,
            failed,
            warnings,
        })),
    )
}

// =====================================================================
// PATCH /api/v1/center/global-connection-ip-restrictions/{ns}/{name}/enable
// =====================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchEnableRequest {
    pub enable: bool,
    pub controllers: Vec<String>,
}

/// `PATCH /api/v1/center/global-connection-ip-restrictions/{ns}/{name}/enable`
pub async fn patch_enable(
    State(state): State<ApiState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<PatchEnableRequest>,
) -> (StatusCode, Json<ApiResponse<FanOutResponse>>) {
    let (targets, warnings) = resolve_targets(&state, &req.controllers);

    let path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", ns, name);
    let futs = targets.into_iter().map(|cid| {
        let proxy = state.proxy.clone();
        let ns = ns.clone();
        let name = name.clone();
        let path = path.clone();
        let enable = req.enable;
        async move {
            // GET live data from the Controller to avoid clobbering concurrent writes
            // with a stale Center-cache snapshot.
            let base = match fetch_live_pm_data(&proxy, &cid, &path).await {
                Ok(d) => d,
                Err((code, msg)) => {
                    return ControllerOpResult {
                        controller_id: cid,
                        detail: None,
                        error: Some(msg),
                        status_code: Some(code),
                    }
                }
            };
            let data = GlobalConnectionIpRestrictionData {
                enable,
                active_profile: base.active_profile,
                profiles: base.profiles,
                description: base.description,
            };
            let body = build_pm_json(&ns, &name, &data).into_bytes();
            let mut headers = HashMap::new();
            headers.insert("content-type".to_string(), "application/json".to_string());
            // NOTE: There is still a TOCTOU window between the GET above and this PUT.
            // A concurrent write between the two calls means the last writer wins silently.
            // Eliminating this would require Controller-side ETag/resourceVersion support.
            match proxy.forward(&cid, "PUT".to_string(), path, headers, body).await {
                Ok(r) if r.status_code >= 200 && r.status_code < 300 => ControllerOpResult {
                    controller_id: cid,
                    detail: Some(format!("status {}", r.status_code)),
                    error: None,
                    status_code: Some(r.status_code as u16),
                },
                Ok(r) => ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some(format!("HTTP {}", r.status_code)),
                    status_code: Some(r.status_code as u16),
                },
                Err((status, msg)) => ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some(msg),
                    status_code: Some(status.as_u16()),
                },
            }
        }
    });
    let results = futures::future::join_all(futs).await;
    let (success, failed) = partition(results);
    fed_metrics::record_fanout(
        fed_metrics::labels::fanout_op::PATCH_ENABLE,
        fanout_result_label(success.len(), failed.len()),
    );
    let status = fanout_status(&success, &failed);
    (
        status,
        Json(ApiResponse::ok_body(FanOutResponse {
            success,
            failed,
            warnings,
        })),
    )
}

// =====================================================================
// PATCH /api/v1/center/global-connection-ip-restrictions/{ns}/{name}/active-profile
// =====================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchActiveProfileRequest {
    pub active_profile: String,
    pub controllers: Vec<String>,
}

/// `PATCH /api/v1/center/global-connection-ip-restrictions/{ns}/{name}/active-profile`
pub async fn patch_active_profile(
    State(state): State<ApiState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<PatchActiveProfileRequest>,
) -> (StatusCode, Json<ApiResponse<FanOutResponse>>) {
    let (targets, warnings) = resolve_targets(&state, &req.controllers);

    let path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", ns, name);
    let futs = targets.into_iter().map(|cid| {
        let proxy = state.proxy.clone();
        let ns = ns.clone();
        let name = name.clone();
        let path = path.clone();
        let active_profile = req.active_profile.clone();
        async move {
            // GET live data from the Controller to avoid clobbering concurrent writes
            // with a stale Center-cache snapshot. Profile existence is validated against
            // the live state for the same reason.
            let base = match fetch_live_pm_data(&proxy, &cid, &path).await {
                Ok(d) => d,
                Err((code, msg)) => {
                    return ControllerOpResult {
                        controller_id: cid,
                        detail: None,
                        error: Some(msg),
                        status_code: Some(code),
                    }
                }
            };
            if !base.profiles.contains_key(&active_profile) {
                return ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some(format!("profile '{}' not found on this controller", active_profile)),
                    status_code: Some(400),
                };
            }
            let data = GlobalConnectionIpRestrictionData {
                enable: base.enable,
                active_profile,
                profiles: base.profiles,
                description: base.description,
            };
            let body = build_pm_json(&ns, &name, &data).into_bytes();
            let mut headers = HashMap::new();
            headers.insert("content-type".to_string(), "application/json".to_string());
            // NOTE: There is still a TOCTOU window between the GET above and this PUT.
            // A concurrent write between the two calls means the last writer wins silently.
            // Eliminating this would require Controller-side ETag/resourceVersion support.
            match proxy.forward(&cid, "PUT".to_string(), path, headers, body).await {
                Ok(r) if r.status_code >= 200 && r.status_code < 300 => ControllerOpResult {
                    controller_id: cid,
                    detail: Some(format!("status {}", r.status_code)),
                    error: None,
                    status_code: Some(r.status_code as u16),
                },
                Ok(r) => ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some(format!("HTTP {}", r.status_code)),
                    status_code: Some(r.status_code as u16),
                },
                Err((status, msg)) => ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some(msg),
                    status_code: Some(status.as_u16()),
                },
            }
        }
    });
    let results = futures::future::join_all(futs).await;
    let (success, failed) = partition(results);
    fed_metrics::record_fanout(
        fed_metrics::labels::fanout_op::PATCH_PROFILE,
        fanout_result_label(success.len(), failed.len()),
    );
    let status = fanout_status(&success, &failed);
    (
        status,
        Json(ApiResponse::ok_body(FanOutResponse {
            success,
            failed,
            warnings,
        })),
    )
}

// =====================================================================
// POST /api/v1/center/global-connection-ip-restrictions/{ns}/{name}/sync
// =====================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub source_controller: String,
    pub target_controllers: Vec<String>,
}

/// `POST /api/v1/center/global-connection-ip-restrictions/{ns}/{name}/sync`
///
/// Copy a PM's current state from `source_controller` to `target_controllers`.
/// `target_controllers: ["all"]` expands to all online controllers except the source.
pub async fn sync_global_ip_restriction(
    State(state): State<ApiState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<SyncRequest>,
) -> (StatusCode, Json<ApiResponse<FanOutResponse>>) {
    let pm_key = format!("{}/{}", ns, name);
    let per_ctrl = match state.metadata_store.get_global_ip_restriction(&pm_key) {
        Some(m) => m,
        None => {
            // 404, not 502: Center is the authoritative origin here; no upstream gateway involved.
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::ok_body(FanOutResponse {
                    success: vec![],
                    failed: vec![ControllerOpResult {
                        controller_id: "<lookup>".to_string(),
                        detail: None,
                        error: Some("PM not found".to_string()),
                        status_code: Some(404),
                    }],
                    warnings: vec![],
                })),
            );
        }
    };

    let Some(source_entry) = per_ctrl.get(&req.source_controller) else {
        // 404: source controller has no entry for this PM (not-found, not a gateway failure).
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::ok_body(FanOutResponse {
                success: vec![],
                failed: vec![ControllerOpResult {
                    controller_id: req.source_controller.clone(),
                    detail: None,
                    error: Some("source controller does not have this PM".to_string()),
                    status_code: Some(404),
                }],
                warnings: vec![],
            })),
        );
    };

    // Resolve targets: "all" = all online except source.
    let (targets, warnings) = if req.target_controllers.len() == 1 && req.target_controllers[0] == "all" {
        let online = online_controllers(&state);
        let targets: Vec<_> = online.into_iter().filter(|cid| cid != &req.source_controller).collect();
        (targets, vec![])
    } else {
        (req.target_controllers.clone(), vec![])
    };

    // Reconstruct the PM data from the source ControllerPmEntry.
    let data = GlobalConnectionIpRestrictionData {
        enable: source_entry.enable,
        active_profile: source_entry.active_profile.clone(),
        profiles: source_entry.profiles.clone(),
        description: source_entry.description.clone(),
    };
    let body = build_pm_json(&ns, &name, &data).into_bytes();
    let path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", ns, name);
    let results = fan_out_http(&state, targets, "PUT", path, body).await;
    let (success, failed) = partition(results);
    let status = fanout_status(&success, &failed);
    (
        status,
        Json(ApiResponse::ok_body(FanOutResponse {
            success,
            failed,
            warnings,
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::plugin_metadata::ProfileRules;

    fn make_pm_json(data: &GlobalConnectionIpRestrictionData) -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "PluginMetaData",
            "metadata": { "name": "test", "namespace": "default" },
            "spec": {
                "metadata": {
                    "type": "GlobalConnectionIpRestriction",
                    "config": serde_json::to_value(data).unwrap()
                }
            }
        })
    }

    fn sample_data() -> GlobalConnectionIpRestrictionData {
        GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "prod".to_string(),
            profiles: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "prod".to_string(),
                    ProfileRules {
                        allow: None,
                        deny: None,
                        default_action: Default::default(),
                        allow_matcher: None,
                        deny_matcher: None,
                    },
                );
                m
            },
            description: Some("test desc".to_string()),
        }
    }

    #[test]
    fn extract_pm_data_success() {
        let v = make_pm_json(&sample_data());
        let got = extract_pm_data(&v).expect("should parse valid PM JSON");
        assert!(got.enable);
        assert_eq!(got.active_profile, "prod");
        assert!(got.profiles.contains_key("prod"));
        assert_eq!(got.description.as_deref(), Some("test desc"));
    }

    #[test]
    fn extract_pm_data_missing_spec() {
        let v = serde_json::json!({ "apiVersion": "edgion.io/v1", "kind": "PluginMetaData" });
        assert!(extract_pm_data(&v).is_err());
    }

    #[test]
    fn extract_pm_data_invalid_config() {
        let v = serde_json::json!({
            "spec": { "metadata": { "config": "not-an-object" } }
        });
        assert!(extract_pm_data(&v).is_err());
    }
}
