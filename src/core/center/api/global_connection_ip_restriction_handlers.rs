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

fn split_pm_key(key: &str) -> (String, String) {
    let mut parts = key.splitn(2, '/');
    let ns = parts.next().unwrap_or("").to_string();
    let name = parts.next().unwrap_or("").to_string();
    (ns, name)
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
    let raw = state.metadata_store.list_global_ip_restrictions();
    let online = online_controllers(&state);
    let items: Vec<_> = raw
        .into_iter()
        .map(|(pm_key, per_ctrl)| {
            let (namespace, name) = split_pm_key(&pm_key);
            CenterGlobalIpRestrictionView {
                namespace,
                name,
                controllers: per_ctrl,
                online_controller_ids: online.clone(),
            }
        })
        .collect();
    Json(ApiResponse::success(items))
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
            Json(ApiResponse::error(format!(
                "GlobalConnectionIpRestriction {}/{} not found",
                ns, name
            ))),
        ));
    };
    let online = online_controllers(&state);
    Ok(Json(ApiResponse::success(CenterGlobalIpRestrictionView {
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
    let raw = state.metadata_store.list_global_ip_restrictions();
    let items: Vec<_> = raw
        .into_iter()
        .map(|(pm_key, per_ctrl)| {
            let (namespace, name) = split_pm_key(&pm_key);
            let hashes: HashSet<String> =
                per_ctrl.values().map(|e| e.content_hash.clone()).collect();
            let consistent = hashes.len() <= 1;
            // v1: report whole-entry mismatch; per-field conflicts left to v2
            let conflicts = if consistent { Vec::new() } else { vec!["contentHash".to_string()] };
            ConsistencyResult {
                namespace,
                name,
                consistent,
                controller_count: per_ctrl.len(),
                conflicts,
            }
        })
        .collect();
    Json(ApiResponse::success(items))
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

// =====================================================================
// POST /api/v1/center/global-connection-ip-restrictions
// =====================================================================

use crate::types::resources::plugin_metadata::GlobalConnectionIpRestrictionData;

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
) -> Json<ApiResponse<FanOutResponse>> {
    // Early validation: catch malformed data before fan-out.
    {
        let mut cloned = req.data.clone();
        if let Err(e) = cloned.validate_and_init() {
            let single = ControllerOpResult {
                controller_id: "<validation>".to_string(),
                detail: None,
                error: Some(format!("invalid PM data: {}", e)),
                status_code: Some(400),
            };
            return Json(ApiResponse::success(FanOutResponse {
                success: vec![],
                failed: vec![single],
                warnings: vec![],
            }));
        }
    }

    let (targets, warnings) = resolve_targets(&state, &req.controllers);

    let pm_json = build_pm_json(&req.namespace, &req.name, &req.data);
    let body = pm_json.into_bytes();
    // PUT /api/v1/namespaced/pluginmetadata/{namespace}/{name} — upsert on Controller
    let path = format!(
        "/api/v1/namespaced/pluginmetadata/{}/{}",
        req.namespace, req.name
    );
    let results = fan_out_http(&state, targets, "PUT", path, body).await;
    let (success, failed) = partition(results);
    Json(ApiResponse::success(FanOutResponse {
        success,
        failed,
        warnings,
    }))
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
) -> Json<ApiResponse<FanOutResponse>> {
    {
        let mut cloned = req.data.clone();
        if let Err(e) = cloned.validate_and_init() {
            return Json(ApiResponse::success(FanOutResponse {
                success: vec![],
                failed: vec![ControllerOpResult {
                    controller_id: "<validation>".to_string(),
                    detail: None,
                    error: Some(format!("invalid PM data: {}", e)),
                    status_code: Some(400),
                }],
                warnings: vec![],
            }));
        }
    }
    let (targets, warnings) = resolve_targets(&state, &req.controllers);
    let body = build_pm_json(&ns, &name, &req.data).into_bytes();
    let path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", ns, name);
    let results = fan_out_http(&state, targets, "PUT", path, body).await;
    let (success, failed) = partition(results);
    Json(ApiResponse::success(FanOutResponse { success, failed, warnings }))
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
) -> Json<ApiResponse<FanOutResponse>> {
    let (targets, warnings) = resolve_targets(&state, &req.controllers);
    let path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", ns, name);
    let results = fan_out_http(&state, targets, "DELETE", path, vec![]).await;
    let (success, failed) = partition(results);
    Json(ApiResponse::success(FanOutResponse { success, failed, warnings }))
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
) -> Json<ApiResponse<FanOutResponse>> {
    let (targets, warnings) = resolve_targets(&state, &req.controllers);
    let pm_key = format!("{}/{}", ns, name);
    let per_ctrl = state
        .metadata_store
        .get_global_ip_restriction(&pm_key)
        .unwrap_or_default();

    let path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", ns, name);
    let futs = targets.into_iter().map(|cid| {
        let base = per_ctrl.get(&cid).cloned();
        let proxy = state.proxy.clone();
        let ns = ns.clone();
        let name = name.clone();
        let path = path.clone();
        let enable = req.enable;
        async move {
            let Some(base) = base else {
                return ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some("PM not found on this controller".to_string()),
                    status_code: Some(404),
                };
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
    Json(ApiResponse::success(FanOutResponse { success, failed, warnings }))
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
) -> Json<ApiResponse<FanOutResponse>> {
    let (targets, warnings) = resolve_targets(&state, &req.controllers);
    let pm_key = format!("{}/{}", ns, name);
    let per_ctrl = state
        .metadata_store
        .get_global_ip_restriction(&pm_key)
        .unwrap_or_default();

    let path = format!("/api/v1/namespaced/pluginmetadata/{}/{}", ns, name);
    let futs = targets.into_iter().map(|cid| {
        let base = per_ctrl.get(&cid).cloned();
        let proxy = state.proxy.clone();
        let ns = ns.clone();
        let name = name.clone();
        let path = path.clone();
        let active_profile = req.active_profile.clone();
        async move {
            let Some(base) = base else {
                return ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some("PM not found on this controller".to_string()),
                    status_code: Some(404),
                };
            };
            if !base.profiles.contains_key(&active_profile) {
                return ControllerOpResult {
                    controller_id: cid,
                    detail: None,
                    error: Some(format!(
                        "profile '{}' not found on this controller",
                        active_profile
                    )),
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
    Json(ApiResponse::success(FanOutResponse { success, failed, warnings }))
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
) -> Json<ApiResponse<FanOutResponse>> {
    let pm_key = format!("{}/{}", ns, name);
    let per_ctrl = match state.metadata_store.get_global_ip_restriction(&pm_key) {
        Some(m) => m,
        None => {
            return Json(ApiResponse::success(FanOutResponse {
                success: vec![],
                failed: vec![ControllerOpResult {
                    controller_id: "<lookup>".to_string(),
                    detail: None,
                    error: Some("PM not found".to_string()),
                    status_code: Some(404),
                }],
                warnings: vec![],
            }));
        }
    };

    let Some(source_entry) = per_ctrl.get(&req.source_controller) else {
        return Json(ApiResponse::success(FanOutResponse {
            success: vec![],
            failed: vec![ControllerOpResult {
                controller_id: req.source_controller.clone(),
                detail: None,
                error: Some("source controller does not have this PM".to_string()),
                status_code: Some(404),
            }],
            warnings: vec![],
        }));
    };

    // Resolve targets: "all" = all online except source.
    let (targets, warnings) = if req.target_controllers.len() == 1 && req.target_controllers[0] == "all" {
        let online = online_controllers(&state);
        let targets: Vec<_> = online
            .into_iter()
            .filter(|cid| cid != &req.source_controller)
            .collect();
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
    Json(ApiResponse::success(FanOutResponse { success, failed, warnings }))
}
