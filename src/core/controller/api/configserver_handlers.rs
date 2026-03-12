//! ConfigServer handlers for edgion-ctl `--target server` support
//!
//! These handlers provide read-only access to ConfigSyncServer cache data,
//! with response format compatible with Gateway's `/configclient/` API.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::common::parse_kind;
use super::types::AdminState;

/// Query parameters for resource lookup
#[derive(Deserialize)]
pub struct ResourceQuery {
    pub namespace: Option<String>,
    pub name: Option<String>,
}

/// Standard API response format (compatible with Gateway)
#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

/// List response format (compatible with Gateway)
#[derive(Serialize)]
pub struct ListResponse {
    pub success: bool,
    pub data: Vec<serde_json::Value>,
    pub count: usize,
}

impl ListResponse {
    pub fn success(data: Vec<serde_json::Value>) -> Self {
        let count = data.len();
        Self {
            success: true,
            data,
            count,
        }
    }
}

/// List all resources of a kind from ConfigSyncServer cache
/// GET /configserver/{kind}/list
pub async fn list_resources(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse>, StatusCode> {
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
        return Ok(Json(ApiResponse::error("Missing required parameter: name".to_string())));
    };

    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Use AdminState's cache methods (reads from ServerCache)
    let resource = match &query.namespace {
        Some(ns) => state.cache_get_resource(kind, ns, &name)?,
        None => state.cache_get_cluster_resource(kind, &name)?,
    };

    match resource {
        Some(r) => Ok(Json(ApiResponse::success(r))),
        None => Ok(Json(ApiResponse::error(format!("{} not found", kind_str)))),
    }
}
