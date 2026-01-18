use crate::types::prelude_resources::*;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use std::sync::Arc;

use super::common::*;
use super::types::*;

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

    // In K8s mode, write operations are not supported via Admin API
    if state.is_k8s_mode() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let writer = state.writer();
    let config_server = state.config_server()?;
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    let metadata = crate::core::utils::extract_resource_metadata(&content).ok_or(StatusCode::BAD_REQUEST)?;
    let name = metadata.name.ok_or(StatusCode::BAD_REQUEST)?;

    // Check if resource already exists in ConfigServer
    let exists = {
        match kind {
            crate::types::ResourceKind::GatewayClass => {
                use kube::ResourceExt;
                let list = config_server.list_gateway_classes();
                list.data.iter().any(|gc| gc.name_any() == name)
            }
            crate::types::ResourceKind::EdgionGatewayConfig => {
                use kube::ResourceExt;
                let list = config_server.list_edgion_gateway_configs();
                list.data.iter().any(|cfg| cfg.name_any() == name)
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        }
    };

    if exists {
        tracing::warn!(
            component = "unified_api",
            event = "resource_already_exists",
            kind = %kind_str,
            name = %name,
            "Cluster resource already exists"
        );
        return Err(StatusCode::CONFLICT);
    }

    // Parse, validate, and persist to writer only (FileWatcher will update ConfigServer)
    match kind {
        crate::types::ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &gc)?;
            let json_content = serde_json::to_string(&gc).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, None, &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EdgionGatewayConfig => {
            let cfg: EdgionGatewayConfig = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &cfg)?;
            let json_content = serde_json::to_string(&cfg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, None, &name, json_content)
                .await
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

    // In K8s mode, write operations are not supported via Admin API
    if state.is_k8s_mode() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let writer = state.writer();
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Parse, validate, and persist to writer only (FileWatcher will update ConfigServer)
    match kind {
        crate::types::ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &gc)?;
            let json_content = serde_json::to_string(&gc).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, None, &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EdgionGatewayConfig => {
            let cfg: EdgionGatewayConfig = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &cfg)?;
            let json_content = serde_json::to_string(&cfg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, None, &name, json_content)
                .await
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

    // In K8s mode, write operations are not supported via Admin API
    if state.is_k8s_mode() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let writer = state.writer();
    let config_server = state.config_server()?;

    // Check if resource exists in ConfigServer
    let exists = {
        match kind {
            crate::types::ResourceKind::GatewayClass => {
                use kube::ResourceExt;
                let list = config_server.list_gateway_classes();
                list.data.iter().any(|gc| gc.name_any() == name)
            }
            crate::types::ResourceKind::EdgionGatewayConfig => {
                use kube::ResourceExt;
                let list = config_server.list_edgion_gateway_configs();
                list.data.iter().any(|cfg| cfg.name_any() == name)
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        }
    };

    if !exists {
        tracing::warn!(
            component = "unified_api",
            event = "resource_not_found",
            kind = %kind_str,
            name = %name,
            "Cluster resource not found"
        );
        return Err(StatusCode::NOT_FOUND);
    }

    // Delete from persistence (FileWatcher will update ConfigServer)
    let _ = writer.delete_one(&kind_str, None, &name).await;

    tracing::info!(
        component = "unified_api",
        event = "cluster_resource_deleted",
        kind = %kind_str,
        name = %name,
        "Cluster resource deleted from persistence"
    );

    Ok(Json(ApiResponse::success(format!("{} deleted", kind_str))))
}
