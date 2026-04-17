//! HTTP handlers for /api/v1/center/global-connection-ip-restrictions endpoints.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
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
