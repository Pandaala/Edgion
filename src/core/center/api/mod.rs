//! Admin HTTP API for edgion-center.
//!
//! Listeners:
//!   - Admin API (http_addr / 12201):  business routes + auth middleware
//!   - Probe     (probe_addr / 12200): GET /health, GET /ready (no auth)
//!   - Metrics   (metrics_addr / 12290): GET /metrics (no auth)
//!
//! Admin routes:
//!   GET  /api/v1/server-info                              → {"mode":"center"}
//!   GET  /api/v1/controllers                              → list all controller summaries
//!   GET  /api/v1/clusters                                 → list distinct cluster names
//!   POST /api/v1/controllers/{id}/reload                  → send reload command
//!   GET  /api/v1/center/cluster-region-routes                      → aggregated ClusterRegionRoute list from MetaDataStore
//!   GET  /api/v1/center/service-region-routes                      → aggregated ServiceRegionRoute list from MetaDataStore
//!   POST /api/v1/center/cluster-region-routes/failover             → fan-out failover to all online controllers (new-URL path)
//!   POST /api/v1/center/service-region-routes/failover             → fan-out failover to all online controllers (new-URL path)
//!   GET  /api/v1/center/cluster-region-routes/consistency          → consistency detection across controllers
//!   GET  /api/v1/center/service-region-routes/consistency          → consistency detection across controllers
//!   GET    /api/v1/center/global-connection-ip-restrictions                        → aggregated GlobalConnectionIpRestriction list from MetaDataStore
//!   POST   /api/v1/center/global-connection-ip-restrictions                        → create/upsert GlobalConnectionIpRestriction (fan-out PUT to target controllers)
//!   GET    /api/v1/center/global-connection-ip-restrictions/{ns}/{name}            → single GlobalConnectionIpRestriction detail
//!   PUT    /api/v1/center/global-connection-ip-restrictions/{ns}/{name}            → update GlobalConnectionIpRestriction (fan-out PUT to target controllers)
//!   DELETE /api/v1/center/global-connection-ip-restrictions/{ns}/{name}            → delete GlobalConnectionIpRestriction (fan-out DELETE to target controllers)
//!   PATCH  /api/v1/center/global-connection-ip-restrictions/{ns}/{name}/enable     → flip enable flag (read-modify-write per controller)
//!   PATCH  /api/v1/center/global-connection-ip-restrictions/{ns}/{name}/active-profile → switch active profile (read-modify-write per controller)
//!   POST   /api/v1/center/global-connection-ip-restrictions/{ns}/{name}/sync       → copy PM state from source controller to target controllers
//!   GET    /api/v1/center/global-connection-ip-restrictions/consistency            → consistency detection across controllers
//!   GET  /api/v1/center/admin/watch-status                          → watch cache sync status per controller
//!   GET  /api/v1/center/admin/metadata-store                         → metadata store key summary
//!   ANY  /api/v1/proxy/{controller_id}/*rest                       → proxy HTTP request to controller

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{any, delete, get, patch, post},
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;

mod consistency_handlers;
mod global_connection_ip_restriction_handlers;
mod region_route_handlers;
pub mod web;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::commander::Commander;
use crate::core::center::db::CenterDb;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::metadata_store::CenterMetaDataStore;
use crate::core::center::proxy::ProxyForwarder;
use crate::core::center::watch_cache::CenterSyncClient;
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
    pub sync_client: Arc<CenterSyncClient>,
    /// Needed by Admin DELETE to cascade eviction into the fed-sync registry.
    /// MetaDataStore is cleaned via `sync_client.plugin_metadata.remove_controller`
    /// (triggers `CenterConfHandler::controller_removed`), not directly.
    pub registry: ControllerRegistry,
    /// True when the database was explicitly configured (`database.enabled = true`).
    /// Used by the `/ready` probe: if the DB was required but failed to open
    /// (`db` is `None`), Center is not ready to serve DB-backed requests.
    pub db_required: bool,
}

impl ApiState {
    /// Returns `true` when Center is ready to handle all API requests.
    ///
    /// Center is considered not ready when the database was explicitly
    /// configured (`database.enabled = true`) but failed to open at startup,
    /// leaving DB-backed endpoints (`list_admin_controllers`,
    /// `delete_admin_controller`) permanently unavailable.
    pub fn is_ready(&self) -> bool {
        !self.db_required || self.db.is_some()
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        // Center-specific endpoints
        .route("/api/v1/server-info", get(server_info))
        .route("/api/v1/controllers", get(list_controllers))
        .route("/api/v1/clusters", get(list_clusters))
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
        // Config sync endpoints
        .route(
            "/api/v1/center/cluster-region-routes/sync",
            post(region_route_handlers::cluster_region_route_sync),
        )
        .route(
            "/api/v1/center/service-region-routes/sync",
            post(region_route_handlers::service_region_route_sync),
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
        // GlobalConnectionIpRestriction endpoints
        .route(
            "/api/v1/center/global-connection-ip-restrictions",
            get(global_connection_ip_restriction_handlers::list_global_ip_restrictions)
                .post(global_connection_ip_restriction_handlers::create_global_ip_restriction),
        )
        .route(
            "/api/v1/center/global-connection-ip-restrictions/{ns}/{name}",
            get(global_connection_ip_restriction_handlers::get_global_ip_restriction)
                .put(global_connection_ip_restriction_handlers::update_global_ip_restriction)
                .delete(global_connection_ip_restriction_handlers::delete_global_ip_restriction),
        )
        .route(
            "/api/v1/center/global-connection-ip-restrictions/{ns}/{name}/enable",
            patch(global_connection_ip_restriction_handlers::patch_enable),
        )
        .route(
            "/api/v1/center/global-connection-ip-restrictions/{ns}/{name}/active-profile",
            patch(global_connection_ip_restriction_handlers::patch_active_profile),
        )
        .route(
            "/api/v1/center/global-connection-ip-restrictions/{ns}/{name}/sync",
            post(global_connection_ip_restriction_handlers::sync_global_ip_restriction),
        )
        .route(
            "/api/v1/center/global-connection-ip-restrictions/consistency",
            get(global_connection_ip_restriction_handlers::global_ip_restrictions_consistency),
        )
        // Admin endpoints (DB-backed)
        .route("/api/v1/center/admin/controllers", get(list_admin_controllers))
        .route("/api/v1/center/admin/controllers/{id}", delete(delete_admin_controller))
        // Watch cache admin endpoints
        .route("/api/v1/center/admin/watch-status", get(watch_status))
        .route("/api/v1/center/admin/metadata-store", get(metadata_store_status))
        // HTTP proxy to controllers
        .route("/api/v1/proxy/{controller_id}/{*rest}", any(proxy_handler))
        .with_state(state)
}

/// Dedicated liveness/readiness router (own listener).
pub fn create_probe_router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/ready", get(ready_check))
        .with_state(state)
}

/// Dedicated Prometheus metrics router (own listener, stateless handler).
pub fn create_metrics_router() -> Router {
    Router::new().route(
        "/metrics",
        get(crate::core::common::observe::metrics_api::metrics_handler),
    )
}

async fn health_check() -> impl IntoResponse {
    Json(ApiResponse::ok_body("ok".to_string()))
}

/// Readiness check endpoint - returns 200 OK only when Center is fully operational.
///
/// Returns 503 when `database.enabled = true` but the SQLite database failed to open
/// at startup, leaving DB-backed endpoints permanently degraded.
async fn ready_check(State(state): State<ApiState>) -> impl IntoResponse {
    if state.is_ready() {
        (StatusCode::OK, Json(ApiResponse::ok_body("ok".to_string())))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::<String>::err_body(
                "database required but unavailable; Center is not ready".to_string(),
            )),
        )
    }
}

#[derive(Serialize)]
struct ServerInfoResponse {
    mode: String,
}

async fn server_info() -> impl IntoResponse {
    Json(ApiResponse::ok_body(ServerInfoResponse {
        mode: "center".to_string(),
    }))
}

async fn list_controllers(State(state): State<ApiState>) -> impl IntoResponse {
    // The aggregator owns stats (key_count, stats_updated_secs_ago) but not
    // session liveness. Fill in `last_seen_secs_ago` from the fed_sync
    // registry here so we keep the aggregator independent of the registry.
    let mut summaries = state.aggregator.controller_summaries();
    for s in summaries.iter_mut() {
        if let Some(session) = state.registry.get_session(&s.controller_id) {
            s.last_seen_secs_ago = Some(session.last_seen.elapsed().as_secs());
        }
    }
    Json(ListResponse::success(summaries))
}

async fn list_clusters(State(state): State<ApiState>) -> impl IntoResponse {
    let summaries = state.aggregator.controller_summaries();
    let mut clusters: Vec<String> = summaries.iter().map(|s| s.cluster.clone()).collect();
    clusters.sort();
    clusters.dedup();
    Json(ListResponse::success(clusters))
}

async fn reload_controller(State(state): State<ApiState>, Path(id_raw): Path<String>) -> impl IntoResponse {
    let id = id_raw.replace('~', "/");
    match state
        .commander
        .send_command(&id, Command::Reload(ReloadCommand {}))
        .await
    {
        Ok(resp) if resp.success => (StatusCode::OK, Json(ApiResponse::ok_body("ok".to_string()))),
        Ok(resp) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::err_body(resp.message)),
        ),
        Err(e) => {
            let status = if e.to_string().contains("timed out") {
                StatusCode::GATEWAY_TIMEOUT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(ApiResponse::<String>::err_body(e.to_string())))
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

impl From<crate::core::center::db::DbController> for AdminControllerDto {
    fn from(r: crate::core::center::db::DbController) -> Self {
        Self {
            controller_id: r.controller_id,
            cluster: r.cluster,
            env: r.env,
            tag: r.tag,
            online: r.online,
            last_seen_at: r.last_seen_at,
        }
    }
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
            let dtos: Vec<AdminControllerDto> = rows.into_iter().map(AdminControllerDto::from).collect();
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
            Json(ApiResponse::<String>::err_body("Database not enabled".to_string())),
        )
            .into_response();
    };
    let id = id_raw.replace('~', "/");

    // Cascade in-memory eviction BEFORE the DB row is deleted. Memory first so
    // a mid-operation crash doesn't leave the DB clean while in-memory state
    // still holds stale entries. `sync_client.plugin_metadata.remove_controller`
    // additionally invokes `CenterConfHandler::controller_removed` on the
    // metadata store, clearing its cluster_routes / service_routes /
    // global_ip_restrictions entries for this controller.
    let removed_registry = state.registry.remove(&id);
    let removed_aggregator = state.aggregator.remove(&id);
    state.sync_client.plugin_metadata.remove_controller(&id);
    tracing::info!(
        component = "admin_api",
        controller_id = %id,
        registry_had_entry = removed_registry,
        aggregator_had_entry = removed_aggregator,
        "DELETE /admin/controllers: evicted in-memory state; proceeding to DB delete"
    );

    let db2 = db.clone();
    let id_for_db = id.clone();
    match tokio::task::spawn_blocking(move || db2.delete_controller(&id_for_db)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::err_body(e.to_string())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<String>::err_body(e.to_string())),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchControllerStatus {
    controller_id: String,
    sync_version: u64,
    server_id: String,
}

async fn watch_status(State(state): State<ApiState>) -> impl IntoResponse {
    let entries = state.sync_client.plugin_metadata.list_controllers();
    let dtos: Vec<WatchControllerStatus> = entries
        .into_iter()
        .map(|(id, ver, sid)| WatchControllerStatus {
            controller_id: id,
            sync_version: ver,
            server_id: sid,
        })
        .collect();
    Json(ListResponse::success(dtos))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetaDataStoreStatus {
    cluster_routes: Vec<MetaDataStoreEntry>,
    service_routes: Vec<MetaDataStoreEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetaDataStoreEntry {
    pm_key: String,
    controller_count: usize,
}

async fn metadata_store_status(State(state): State<ApiState>) -> impl IntoResponse {
    let cluster_routes: Vec<MetaDataStoreEntry> = state
        .metadata_store
        .list_cluster_routes()
        .into_iter()
        .map(|v| MetaDataStoreEntry {
            pm_key: format!("{}/{}", v.namespace, v.name),
            controller_count: v.controllers.len(),
        })
        .collect();
    let service_routes: Vec<MetaDataStoreEntry> = state
        .metadata_store
        .list_service_routes()
        .into_iter()
        .map(|v| MetaDataStoreEntry {
            pm_key: format!("{}/{}", v.namespace, v.name),
            controller_count: v.controllers.len(),
        })
        .collect();
    Json(ApiResponse::ok_body(MetaDataStoreStatus {
        cluster_routes,
        service_routes,
    }))
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
                    serde_json::to_vec(&ApiResponse::<()>::err_body(message)).unwrap_or_default(),
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
