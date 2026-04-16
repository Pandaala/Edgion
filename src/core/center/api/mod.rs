//! Admin HTTP API for edgion-center.
//!
//! Routes:
//!   GET  /health                                          → health check
//!   GET  /ready                                           → readiness check
//!   GET  /api/v1/server-info                              → {"mode":"center"}
//!   GET  /api/v1/controllers                              → list all controller summaries
//!   GET  /api/v1/clusters                                 → list distinct cluster names
//!   GET  /api/v1/resources                                → list resource keys (query: cluster=, kind=)
//!   POST /api/v1/controllers/{id}/reload                  → send reload command
//!   GET  /api/v1/center/cluster-region-routes                      → aggregated ClusterRegionRoute list from MetaDataStore
//!   GET  /api/v1/center/service-region-routes                      → aggregated ServiceRegionRoute list from MetaDataStore
//!   POST /api/v1/center/cluster-region-routes/failover             → fan-out failover to all online controllers (new-URL path)
//!   POST /api/v1/center/service-region-routes/failover             → fan-out failover to all online controllers (new-URL path)
//!   GET  /api/v1/center/cluster-region-routes/consistency          → consistency detection across controllers
//!   GET  /api/v1/center/service-region-routes/consistency          → consistency detection across controllers
//!   ANY  /api/v1/proxy/{controller_id}/*rest                       → proxy HTTP request to controller

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{any, delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod consistency_handlers;
mod region_route_handlers;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::commander::Commander;
use crate::core::center::db::CenterDb;
use crate::core::center::metadata_store::CenterMetaDataStore;
use crate::core::center::proxy::ProxyForwarder;
use crate::core::common::api::{ApiResponse, ListResponse};
use crate::core::common::fed_sync::proto::command_request::Command;
use crate::core::common::fed_sync::proto::ReloadCommand;

#[derive(Clone)]
pub struct ApiState {
    pub aggregator: Arc<ResourceAggregator>,
    pub commander: Arc<Commander>,
    pub proxy: Arc<ProxyForwarder>,
    pub db: Option<Arc<CenterDb>>,
    pub metadata_store: Arc<CenterMetaDataStore>,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        // Health/ready (no /api/v1 prefix, consistent with controller)
        .route("/health", get(health_check))
        .route("/ready", get(ready_check))
        // Center-specific endpoints
        .route("/api/v1/server-info", get(server_info))
        .route("/api/v1/controllers", get(list_controllers))
        .route("/api/v1/clusters", get(list_clusters))
        .route("/api/v1/resources", get(list_resources))
        .route("/api/v1/controllers/{id}/reload", post(reload_controller))
        // MetaDataStore-backed RegionRoute endpoints
        .route(
            "/api/v1/center/cluster-region-routes",
            get(region_route_handlers::list_cluster_region_routes),
        )
        .route(
            "/api/v1/center/service-region-routes",
            get(region_route_handlers::list_service_region_routes),
        )
        // RegionRoute failover (new-URL paths)
        .route(
            "/api/v1/center/cluster-region-routes/failover",
            post(region_route_handlers::cluster_region_route_failover),
        )
        .route(
            "/api/v1/center/service-region-routes/failover",
            post(region_route_handlers::service_region_route_failover),
        )
        // Consistency detection endpoints
        .route(
            "/api/v1/center/cluster-region-routes/consistency",
            get(consistency_handlers::cluster_region_routes_consistency),
        )
        .route(
            "/api/v1/center/service-region-routes/consistency",
            get(consistency_handlers::service_region_routes_consistency),
        )
        // Admin endpoints (DB-backed)
        .route("/api/v1/center/admin/controllers", get(list_admin_controllers))
        .route("/api/v1/center/admin/controllers/{id}", delete(delete_admin_controller))
        // HTTP proxy to controllers
        .route("/api/v1/proxy/{controller_id}/{*rest}", any(proxy_handler))
        .with_state(state)
}

async fn health_check() -> impl IntoResponse {
    Json(ApiResponse::success("ok".to_string()))
}

async fn ready_check() -> impl IntoResponse {
    Json(ApiResponse::success("ok".to_string()))
}

#[derive(Serialize)]
struct ServerInfoResponse {
    mode: String,
}

async fn server_info() -> impl IntoResponse {
    Json(ApiResponse::success(ServerInfoResponse {
        mode: "center".to_string(),
    }))
}

async fn list_controllers(State(state): State<ApiState>) -> impl IntoResponse {
    let summaries = state.aggregator.controller_summaries();
    Json(ListResponse::success(summaries))
}

async fn list_clusters(State(state): State<ApiState>) -> impl IntoResponse {
    let summaries = state.aggregator.controller_summaries();
    let mut clusters: Vec<String> = summaries.iter().map(|s| s.cluster.clone()).collect();
    clusters.sort();
    clusters.dedup();
    Json(ListResponse::success(clusters))
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
    Json(ListResponse::success(keys))
}

async fn reload_controller(State(state): State<ApiState>, Path(id_raw): Path<String>) -> impl IntoResponse {
    let id = id_raw.replace('~', "/");
    match state
        .commander
        .send_command(&id, Command::Reload(ReloadCommand {}))
        .await
    {
        Ok(resp) if resp.success => (StatusCode::OK, Json(ApiResponse::success("ok".to_string()))),
        Ok(resp) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(resp.message)),
        ),
        Err(e) => {
            let status = if e.to_string().contains("timed out") {
                StatusCode::GATEWAY_TIMEOUT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(ApiResponse::<String>::error(e.to_string())))
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminControllerDto {
    controller_id: String,
    cluster: String,
    env: Vec<String>,
    tag: Vec<String>,
    online: bool,
    last_seen_at: i64,
}

async fn list_admin_controllers(State(state): State<ApiState>) -> impl IntoResponse {
    let Some(db) = &state.db else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ListResponse::<AdminControllerDto>::error(
                "Database not enabled".to_string(),
            )),
        )
            .into_response();
    };
    let db2 = db.clone();
    match tokio::task::spawn_blocking(move || db2.list_controllers()).await {
        Ok(Ok(rows)) => {
            let dtos: Vec<AdminControllerDto> = rows
                .into_iter()
                .map(|r| AdminControllerDto {
                    controller_id: r.controller_id,
                    cluster: r.cluster,
                    env: r.env,
                    tag: r.tag,
                    online: r.online,
                    last_seen_at: r.last_seen_at,
                })
                .collect();
            Json(ListResponse::success(dtos)).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ListResponse::<AdminControllerDto>::error(e.to_string())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ListResponse::<AdminControllerDto>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn delete_admin_controller(State(state): State<ApiState>, Path(id_raw): Path<String>) -> impl IntoResponse {
    let Some(db) = &state.db else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::<String>::error("Database not enabled".to_string())),
        )
            .into_response();
    };
    let id = id_raw.replace('~', "/");
    let db2 = db.clone();
    match tokio::task::spawn_blocking(move || db2.delete_controller(&id)).await {
        Ok(Ok(())) => Json(ApiResponse::success("ok".to_string())).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(e.to_string())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn proxy_handler(
    State(state): State<ApiState>,
    Path((controller_id_raw, rest)): Path<(String, String)>,
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // Frontend uses "~" instead of "/" in controller_id to avoid browser URL decoding issues
    let controller_id = controller_id_raw.replace('~', "/");

    // Convert headers to HashMap<String, String>, skipping non-UTF-8 values
    let headers_map: std::collections::HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|val| (k.as_str().to_string(), val.to_string())))
        .collect();

    // Ensure path starts with / for forwarding
    let path = if rest.starts_with('/') {
        rest
    } else {
        format!("/{}", rest)
    };

    match state
        .proxy
        .forward(&controller_id, method.to_string(), path, headers_map, body.to_vec())
        .await
    {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status_code as u16).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

            let mut builder = axum::http::Response::builder().status(status);

            for (key, value) in &resp.headers {
                builder = builder.header(key.as_str(), value.as_str());
            }

            builder.body(axum::body::Body::from(resp.body)).unwrap_or_else(|_| {
                axum::http::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(axum::body::Body::empty())
                    .unwrap()
            })
        }
        Err((status, message)) => {
            tracing::warn!(
                component = "center",
                controller_id = %controller_id,
                status = %status,
                error = %message,
                "Proxy request failed"
            );
            axum::http::Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&ApiResponse::<()>::error(message)).unwrap_or_default(),
                ))
                .unwrap_or_else(|_| {
                    axum::http::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(axum::body::Body::empty())
                        .unwrap()
                })
        }
    }
}
