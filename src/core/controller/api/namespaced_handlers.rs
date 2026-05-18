use crate::core::controller::conf_mgr::ListOptions;
use crate::types::prelude_resources::*;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use k8s_openapi::api::core::v1::{ConfigMap, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use serde::Deserialize;
use std::sync::Arc;

use super::common::{map_writer_error, parse_kind, parse_resource_and_update_version, validate_resource};
use super::types::*;

/// Pagination query parameters for list endpoints.
#[derive(Deserialize, Default)]
pub struct PaginationQuery {
    /// Maximum number of items to return (0 or absent = no limit).
    pub limit: Option<u32>,
    /// Continue token from a previous list response.
    #[serde(rename = "continue")]
    pub continue_token: Option<String>,
}

impl PaginationQuery {
    pub(super) fn into_list_options(self) -> Option<ListOptions> {
        if self.limit.is_some() || self.continue_token.is_some() {
            Some(ListOptions {
                limit: self.limit.unwrap_or(0),
                continue_token: self.continue_token,
            })
        } else {
            None
        }
    }
}

/// List all resources of a kind across all namespaces (from ConfCenter/storage)
pub async fn list_all_namespaces(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state
        .center_list_resources(kind, pagination.into_list_options())
        .await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

/// List namespace-scoped resources (from ConfCenter/storage)
pub async fn list_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state
        .center_list_resources_namespaced(kind, &ns, pagination.into_list_options())
        .await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

/// Get a namespace-scoped resource (from ConfCenter/storage)
pub async fn get_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let resource = state.center_get_resource(kind, &ns, &name).await?;
    resource.map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// Create a namespace-scoped resource
pub async fn create_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    tracing::info!(
        component = "unified_api",
        event = "create_request",
        kind = %kind_str,
        namespace = %ns,
        body_len = body.len(),
        "Received create request"
    );

    let kind = parse_kind(&kind_str).map_err(|e| {
        tracing::warn!("Failed to parse kind: {}", e);
        StatusCode::BAD_REQUEST
    })?;

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    let content = String::from_utf8(body.to_vec()).map_err(|e| {
        tracing::warn!("Failed to parse body as UTF-8: {}", e);
        StatusCode::BAD_REQUEST
    })?;

    let metadata = crate::core::common::utils::extract_resource_metadata(&content).ok_or_else(|| {
        tracing::warn!("Failed to extract metadata from request body");
        StatusCode::BAD_REQUEST
    })?;

    let name = metadata.name.ok_or_else(|| {
        tracing::warn!("Resource name is missing in metadata");
        StatusCode::BAD_REQUEST
    })?;

    tracing::info!("Extracted resource name: {}", name);

    // Parse, validate, and persist using create_one (fails if exists)
    // In K8s mode: skip validation (K8s API Server validates) and don't update version
    // In non-K8s mode: validate and update resource version
    match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &service, is_k8s)?;
            let json_content = serde_json::to_string(&service).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &ep, is_k8s)?;
            let json_content = serde_json::to_string(&ep).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &tls, is_k8s)?;
            let json_content = serde_json::to_string(&tls).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &metadata, is_k8s)?;
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &linksys, is_k8s)?;
            let json_content = serde_json::to_string(&linksys).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &secret, is_k8s)?;
            let json_content = serde_json::to_string(&secret).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::ConfigMap => {
            let cm: ConfigMap = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &cm, is_k8s)?;
            let json_content = serde_json::to_string(&cm).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Gateway => {
            let gateway: Gateway = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &gateway, is_k8s)?;
            let json_content = serde_json::to_string(&gateway).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::ReferenceGrant => {
            let rg: ReferenceGrant = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &rg, is_k8s)?;
            let json_content = serde_json::to_string(&rg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::BackendTLSPolicy => {
            let policy: BackendTLSPolicy = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &policy, is_k8s)?;
            let json_content = serde_json::to_string(&policy).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionStreamPlugins => {
            let plugins: EdgionStreamPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionAcme => {
            let acme: EdgionAcme = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &acme, is_k8s)?;
            let json_content = serde_json::to_string(&acme).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    }

    tracing::info!(
        component = "unified_api",
        event = "resource_created",
        kind = %kind_str,
        namespace = %ns,
        name = %name,
        is_k8s_mode = is_k8s,
        "Resource created successfully"
    );

    Ok(Json(ApiResponse::success(format!("{} created", kind_str))))
}

/// Update a namespace-scoped resource
pub async fn update_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Parse, validate, and persist using update_one (fails if not exists)
    // In K8s mode: skip validation (K8s API Server validates) and don't update version
    // In non-K8s mode: validate and update resource version
    match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &service, is_k8s)?;
            let json_content = serde_json::to_string(&service).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &ep, is_k8s)?;
            let json_content = serde_json::to_string(&ep).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &tls, is_k8s)?;
            let json_content = serde_json::to_string(&tls).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &metadata, is_k8s)?;
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &linksys, is_k8s)?;
            let json_content = serde_json::to_string(&linksys).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &secret, is_k8s)?;
            let json_content = serde_json::to_string(&secret).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::ConfigMap => {
            let cm: ConfigMap = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &cm, is_k8s)?;
            let json_content = serde_json::to_string(&cm).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Gateway => {
            let gateway: Gateway = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &gateway, is_k8s)?;
            let json_content = serde_json::to_string(&gateway).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::ReferenceGrant => {
            let rg: ReferenceGrant = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &rg, is_k8s)?;
            let json_content = serde_json::to_string(&rg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::BackendTLSPolicy => {
            let policy: BackendTLSPolicy = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &policy, is_k8s)?;
            let json_content = serde_json::to_string(&policy).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionStreamPlugins => {
            let plugins: EdgionStreamPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionAcme => {
            let acme: EdgionAcme = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &acme, is_k8s)?;
            let json_content = serde_json::to_string(&acme).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    }

    tracing::info!(
        component = "unified_api",
        event = "resource_updated",
        kind = %kind_str,
        namespace = %ns,
        name = %name,
        is_k8s_mode = is_k8s,
        "Resource updated successfully"
    );

    Ok(Json(ApiResponse::success(format!("{} updated", kind_str))))
}

/// Delete a namespace-scoped resource
pub async fn delete_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    // Delete from backend - delete_one will return NotFound if resource doesn't exist
    writer
        .delete_one(kind.as_str(), Some(&ns), &name)
        .await
        .map_err(map_writer_error)?;

    tracing::info!(
        component = "unified_api",
        event = "resource_deleted",
        kind = %kind_str,
        namespace = %ns,
        name = %name,
        is_k8s_mode = is_k8s,
        "Resource deleted successfully"
    );

    Ok(Json(ApiResponse::success(format!("{} deleted", kind_str))))
}

/// List metadata-only keys for a kind across all namespaces (keys endpoint)
pub async fn list_all_namespaces_keys(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ListResponse<crate::core::controller::conf_mgr::conf_center::traits::ResourceKey>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state.center_list_keys(kind, pagination.into_list_options()).await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

/// List metadata-only keys for a kind scoped to a namespace (keys endpoint)
pub async fn list_namespaced_keys(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ListResponse<crate::core::controller::conf_mgr::conf_center::traits::ResourceKey>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state
        .center_list_keys_namespaced(kind, &ns, pagination.into_list_options())
        .await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}
