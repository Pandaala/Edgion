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
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{any, delete, get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod consistency_handlers;
mod global_connection_ip_restriction_handlers;
mod region_route_handlers;

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
    // Default view excludes offline controllers: their snapshots are stale and
    // would mislead consumers into thinking the resources are still live.
    // Operators wanting the raw persisted record can hit `GET /api/v1/center/admin/controllers`.
    let keys = match &params.cluster {
        Some(c) => state.aggregator.list_online_keys_by_cluster(c),
        None => state.aggregator.list_online_keys(),
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
    Json(ApiResponse::success(MetaDataStoreStatus {
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
