use crate::types::prelude_resources::*;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header::LOCATION, HeaderMap, HeaderValue, StatusCode},
    response::Json,
};
use std::sync::Arc;

use super::common::{map_writer_error, parse_kind, parse_resource_and_update_version, validate_resource};
use super::types::*;
use crate::types::resource::is_kind_cluster_scoped;

/// List all cluster-scoped resources of a kind
pub async fn list_cluster(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let (data, continue_token) = state.center_list_resources(kind, None).await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

/// Get a cluster-scoped resource
pub async fn get_cluster(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let resource = state.center_get_cluster_resource(kind, &name).await?;
    resource.map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// Create a cluster-scoped resource.
pub async fn create_cluster(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, HeaderMap, Json<ApiResponse<String>>), StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    let metadata = crate::core::common::utils::extract_resource_metadata(&content).ok_or(StatusCode::BAD_REQUEST)?;
    let name = metadata.name.ok_or(StatusCode::BAD_REQUEST)?;

    // Parse, validate, and persist using create_one (fails if exists)
    // In K8s mode: skip validation (K8s API Server validates) and don't update version
    // In non-K8s mode: validate and update resource version
    match kind {
        crate::types::ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &gc, is_k8s)?;
            let json_content = serde_json::to_string(&gc).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), None, &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionGatewayConfig => {
            let cfg: EdgionGatewayConfig = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &cfg, is_k8s)?;
            let json_content = serde_json::to_string(&cfg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), None, &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    }

    tracing::info!(
        component = "unified_api",
        event = "cluster_resource_created",
        kind = %kind_str,
        name = %name,
        is_k8s_mode = is_k8s,
        "Cluster resource created"
    );

    let location = format!("/api/v1/cluster/{}/{}", kind_str, name);
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::try_from(location) {
        headers.insert(LOCATION, v);
    }
    Ok((
        StatusCode::CREATED,
        headers,
        Json(ApiResponse::ok_body(format!("{} created", kind_str))),
    ))
}

/// Update a cluster-scoped resource
pub async fn update_cluster(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, name)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Parse, validate, and persist using update_one (fails if not exists)
    // In K8s mode: skip validation (K8s API Server validates) and don't update version
    // In non-K8s mode: validate and update resource version
    match kind {
        crate::types::ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &gc, is_k8s)?;
            let json_content = serde_json::to_string(&gc).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), None, &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionGatewayConfig => {
            let cfg: EdgionGatewayConfig = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &cfg, is_k8s)?;
            let json_content = serde_json::to_string(&cfg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), None, &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    }

    tracing::info!(
        component = "unified_api",
        event = "cluster_resource_updated",
        kind = %kind_str,
        name = %name,
        is_k8s_mode = is_k8s,
        "Cluster resource updated"
    );

    Ok(Json(ApiResponse::ok_body(format!("{} updated", kind_str))))
}

/// Delete a cluster-scoped resource.
pub async fn delete_cluster(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, name)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    // Delete from backend - delete_one will return NotFound if resource doesn't exist
    writer
        .delete_one(kind.as_str(), None, &name)
        .await
        .map_err(map_writer_error)?;

    tracing::info!(
        component = "unified_api",
        event = "cluster_resource_deleted",
        kind = %kind_str,
        name = %name,
        is_k8s_mode = is_k8s,
        "Cluster resource deleted"
    );

    Ok(StatusCode::NO_CONTENT)
}

/// List metadata-only keys for a cluster-scoped kind (keys endpoint)
pub async fn list_cluster_keys(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    Query(pagination): Query<super::namespaced_handlers::PaginationQuery>,
) -> Result<Json<ListResponse<crate::core::controller::conf_mgr::conf_center::traits::ResourceKey>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let (data, continue_token) = state.center_list_keys(kind, pagination.into_list_options()).await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}
