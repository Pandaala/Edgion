use axum::{
    extract::{Path, State},
    response::Json,
    body::Bytes,
    http::StatusCode,
};
use std::sync::Arc;
use crate::types::{prelude_resources::*};

use super::types::*;
use super::common::*;

/// List all cluster-scoped resources of a kind
pub async fn list_cluster(
    State(_state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    if !is_cluster_scoped(&kind) {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    // Cluster-scoped resources are typically stored in base_conf or not implemented yet
    Err(StatusCode::NOT_IMPLEMENTED)
}

/// Get a cluster-scoped resource
pub async fn get_cluster(
    State(_state): State<Arc<AdminState>>,
    Path((kind_str, _name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    if !is_cluster_scoped(&kind) {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    // Cluster-scoped resources not yet implemented
    Err(StatusCode::NOT_IMPLEMENTED)
}

/// Create a cluster-scoped resource
pub async fn create_cluster(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    if !is_cluster_scoped(&kind) {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let metadata = crate::core::utils::extract_resource_metadata(&content).ok_or(StatusCode::BAD_REQUEST)?;
    let name = metadata.name.ok_or(StatusCode::BAD_REQUEST)?;
    
    // Parse, validate, and persist
    match kind {
        crate::types::ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &gc)?;
            let json_content = serde_json::to_string(&gc)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, None, &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EdgionGatewayConfig => {
            let cfg: EdgionGatewayConfig = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &cfg)?;
            let json_content = serde_json::to_string(&cfg)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, None, &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    }
    
    tracing::info!(
        component = "unified_api",
        event = "cluster_resource_created",
        kind = %kind_str,
        name = %name,
        "Cluster resource created"
    );
    
    Ok(Json(ApiResponse::success(format!("{} created", kind_str))))
}

/// Update a cluster-scoped resource
pub async fn update_cluster(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, name)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    if !is_cluster_scoped(&kind) {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    // Parse, validate, and persist
    match kind {
        crate::types::ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &gc)?;
            let json_content = serde_json::to_string(&gc)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, None, &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EdgionGatewayConfig => {
            let cfg: EdgionGatewayConfig = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &cfg)?;
            let json_content = serde_json::to_string(&cfg)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, None, &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    }
    
    tracing::info!(
        component = "unified_api",
        event = "cluster_resource_updated",
        kind = %kind_str,
        name = %name,
        "Cluster resource updated"
    );
    
    Ok(Json(ApiResponse::success(format!("{} updated", kind_str))))
}

/// Delete a cluster-scoped resource
pub async fn delete_cluster(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, name)): Path<(String, String)>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    if !is_cluster_scoped(&kind) {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    
    // Read, validate, and delete
    let content = resource_mgr
        .get_one(&kind_str, None, &name)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    
    match kind {
        crate::types::ResourceKind::GatewayClass => {
            let _: GatewayClass = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, None, &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EdgionGatewayConfig => {
            let _: EdgionGatewayConfig = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, None, &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    }
    
    tracing::info!(
        component = "unified_api",
        event = "cluster_resource_deleted",
        kind = %kind_str,
        name = %name,
        "Cluster resource deleted"
    );
    
    Ok(Json(ApiResponse::success(format!("{} deleted", kind_str))))
}

