use crate::core::conf_mgr::conf_center::sync_runtime::resource_processor::check_edgion_tls;
use crate::types::prelude_resources::*;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::sync::Arc;

use super::common::{map_writer_error, parse_kind, parse_resource_and_update_version, validate_resource};
use super::types::*;
use crate::{get_namespaced_resource, list_all_resources, list_namespaced_resources, list_to_json};

/// List all resources of a kind across all namespaces
pub async fn list_all_namespaces(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let config_server = state.config_server()?;
    let data = list_all_resources!(&config_server, kind);
    Ok(Json(ListResponse::success(data)))
}

/// List namespace-scoped resources
pub async fn list_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let config_server = state.config_server()?;
    let data = list_namespaced_resources!(&config_server, kind, ns);
    Ok(Json(ListResponse::success(data)))
}

/// Get a namespace-scoped resource
pub async fn get_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let config_server = state.config_server()?;
    let resource = get_namespaced_resource!(&config_server, kind, ns, name);
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
    let writer = state.writer();

    let content = String::from_utf8(body.to_vec()).map_err(|e| {
        tracing::warn!("Failed to parse body as UTF-8: {}", e);
        StatusCode::BAD_REQUEST
    })?;

    tracing::debug!("Request body: {}", content);

    let metadata = crate::core::utils::extract_resource_metadata(&content).ok_or_else(|| {
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
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &service, is_k8s)?;
            let json_content = serde_json::to_string(&service).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &ep, is_k8s)?;
            let json_content = serde_json::to_string(&ep).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Endpoint => {
            let endpoint: Endpoints = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &endpoint, is_k8s)?;
            let json_content = serde_json::to_string(&endpoint).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &tls, is_k8s)?;

            // Validate EdgionTls before apply (skip in K8s mode)
            if !is_k8s {
                let config_server = state.config_server()?;
                let check_result = check_edgion_tls(&config_server, &tls);

                if let Some(reason) = check_result.skip_reason {
                    tracing::info!(
                        component = "unified_api",
                        kind = "EdgionTls",
                        name = %name,
                        namespace = %ns,
                        reason = %reason,
                        "EdgionTls validation failed"
                    );
                    return Err(StatusCode::UNPROCESSABLE_ENTITY);
                }

                for warning in &check_result.warnings {
                    tracing::warn!(
                        component = "unified_api",
                        kind = "EdgionTls",
                        name = %name,
                        namespace = %ns,
                        warning = %warning,
                        "EdgionTls validation warning"
                    );
                }
            }

            let json_content = serde_json::to_string(&tls).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &metadata, is_k8s)?;
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &linksys, is_k8s)?;
            let json_content = serde_json::to_string(&linksys).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &secret, is_k8s)?;
            let json_content = serde_json::to_string(&secret).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Gateway => {
            let gateway: Gateway = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &gateway, is_k8s)?;
            let json_content = serde_json::to_string(&gateway).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::ReferenceGrant => {
            let rg: ReferenceGrant = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &rg, is_k8s)?;
            let json_content = serde_json::to_string(&rg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::BackendTLSPolicy => {
            let policy: BackendTLSPolicy = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &policy, is_k8s)?;
            let json_content = serde_json::to_string(&policy).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionStreamPlugins => {
            let plugins: EdgionStreamPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    let writer = state.writer();

    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Parse, validate, and persist using update_one (fails if not exists)
    // In K8s mode: skip validation (K8s API Server validates) and don't update version
    // In non-K8s mode: validate and update resource version
    match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &service, is_k8s)?;
            let json_content = serde_json::to_string(&service).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &ep, is_k8s)?;
            let json_content = serde_json::to_string(&ep).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Endpoint => {
            let endpoint: Endpoints = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &endpoint, is_k8s)?;
            let json_content = serde_json::to_string(&endpoint).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &tls, is_k8s)?;

            // Validate EdgionTls before apply (skip in K8s mode)
            if !is_k8s {
                let config_server = state.config_server()?;
                let check_result = check_edgion_tls(&config_server, &tls);

                if let Some(reason) = check_result.skip_reason {
                    tracing::info!(
                        component = "unified_api",
                        kind = "EdgionTls",
                        name = %name,
                        namespace = %ns,
                        reason = %reason,
                        "EdgionTls validation failed on update"
                    );
                    return Err(StatusCode::UNPROCESSABLE_ENTITY);
                }

                for warning in &check_result.warnings {
                    tracing::warn!(
                        component = "unified_api",
                        kind = "EdgionTls",
                        name = %name,
                        namespace = %ns,
                        warning = %warning,
                        "EdgionTls validation warning"
                    );
                }
            }

            let json_content = serde_json::to_string(&tls).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &metadata, is_k8s)?;
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &linksys, is_k8s)?;
            let json_content = serde_json::to_string(&linksys).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &secret, is_k8s)?;
            let json_content = serde_json::to_string(&secret).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::Gateway => {
            let gateway: Gateway = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &gateway, is_k8s)?;
            let json_content = serde_json::to_string(&gateway).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::ReferenceGrant => {
            let rg: ReferenceGrant = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &rg, is_k8s)?;
            let json_content = serde_json::to_string(&rg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::BackendTLSPolicy => {
            let policy: BackendTLSPolicy = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &policy, is_k8s)?;
            let json_content = serde_json::to_string(&policy).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        crate::types::ResourceKind::EdgionStreamPlugins => {
            let plugins: EdgionStreamPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(&state.schema_validator, kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    let writer = state.writer();

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
