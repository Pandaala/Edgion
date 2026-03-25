//! Admin HTTP API for edgion-center.
//!
//! Routes:
//!   GET  /controllers              → list all controller summaries
//!   GET  /clusters                 → list distinct cluster names
//!   GET  /resources                → list resource keys (query: cluster=, kind=)
//!   POST /controllers/{id}/reload  → send reload command

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::commander::Commander;
use crate::core::common::fed_sync::proto::command_request::Command;
use crate::core::common::fed_sync::proto::ReloadCommand;

#[derive(Clone)]
pub struct ApiState {
    pub aggregator: Arc<ResourceAggregator>,
    pub commander: Arc<Commander>,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/controllers", get(list_controllers))
        .route("/clusters", get(list_clusters))
        .route("/resources", get(list_resources))
        .route("/controllers/:id/reload", post(reload_controller))
        .with_state(state)
}

async fn list_controllers(State(state): State<ApiState>) -> impl IntoResponse {
    let summaries = state.aggregator.controller_summaries();
    Json(summaries)
}

async fn list_clusters(State(state): State<ApiState>) -> impl IntoResponse {
    let summaries = state.aggregator.controller_summaries();
    let mut clusters: Vec<String> = summaries.iter().map(|s| s.cluster.clone()).collect();
    clusters.sort();
    clusters.dedup();
    Json(clusters)
}

#[derive(Deserialize)]
struct ResourceQuery {
    cluster: Option<String>,
    kind: Option<String>,
}

/// Serializable DTO for prost-generated ResourceKey (prost types don't impl serde::Serialize).
#[derive(serde::Serialize)]
struct ResourceKeyDto {
    kind: String,
    namespace: String,
    name: String,
    resource_version: String,
    labels: std::collections::HashMap<String, String>,
    annotations: std::collections::HashMap<String, String>,
}

impl From<crate::core::common::fed_sync::proto::ResourceKey> for ResourceKeyDto {
    fn from(k: crate::core::common::fed_sync::proto::ResourceKey) -> Self {
        Self {
            kind: k.kind,
            namespace: k.namespace,
            name: k.name,
            resource_version: k.resource_version,
            labels: k.labels,
            annotations: k.annotations,
        }
    }
}

async fn list_resources(State(state): State<ApiState>, Query(params): Query<ResourceQuery>) -> impl IntoResponse {
    let keys = match &params.cluster {
        Some(c) => state.aggregator.list_keys_by_cluster(c),
        None => state.aggregator.list_all_keys(),
    };
    let keys: Vec<ResourceKeyDto> = match &params.kind {
        Some(k) => keys
            .into_iter()
            .filter(|key| &key.kind == k)
            .map(ResourceKeyDto::from)
            .collect(),
        None => keys.into_iter().map(ResourceKeyDto::from).collect(),
    };
    Json(keys)
}

async fn reload_controller(State(state): State<ApiState>, Path(id): Path<String>) -> impl IntoResponse {
    match state
        .commander
        .send_command(&id, Command::Reload(ReloadCommand {}))
        .await
    {
        Ok(resp) if resp.success => (StatusCode::OK, Json(serde_json::json!({"ok": true}))),
        Ok(resp) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": resp.message})),
        ),
        Err(e) => {
            if e.to_string().contains("timed out") {
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            }
        }
    }
}
