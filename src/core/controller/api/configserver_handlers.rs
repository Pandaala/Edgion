//! ConfigServer handlers for edgion-cli `--target server` support
//!
//! These handlers provide read-only access to ConfigSyncServer cache data,
//! with response format compatible with Gateway's `/configclient/` API.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use std::sync::Arc;

use super::common::parse_kind;
use super::types::{AdminState, ApiResponse, ListResponse};

/// Query parameters for resource lookup
#[derive(Deserialize)]
pub struct ResourceQuery {
    pub namespace: Option<String>,
    pub name: Option<String>,
}

/// List all resources of a kind from ConfigSyncServer cache
/// GET /configserver/{kind}/list
pub async fn list_resources(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let data = state.cache_list_resources(kind)?;
    Ok(Json(ListResponse::success(data)))
}

/// Get a single resource from ConfigSyncServer cache by namespace and name
/// GET /configserver/{kind}?namespace=xxx&name=xxx
pub async fn get_resource(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<ApiResponse<serde_json::Value>>, StatusCode> {
    let Some(name) = query.name else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Use AdminState's cache methods (reads from ServerCache)
    let resource = match &query.namespace {
        Some(ns) => state.cache_get_resource(kind, ns, &name)?,
        None => state.cache_get_cluster_resource(kind, &name)?,
    };

    match resource {
        Some(r) => Ok(Json(ApiResponse::ok_body(r))),
        None => Err(StatusCode::NOT_FOUND),
    }
}
