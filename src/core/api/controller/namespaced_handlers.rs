use crate::core::conf_mgr::resource_check::{check_edgion_tls, ResourceCheckContext};
use crate::types::prelude_resources::*;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::ResourceExt;
use std::sync::Arc;

use super::common::*;
use super::types::*;
use crate::{
    get_namespaced_resource, list_all_resources, list_namespaced_resources, list_to_json, resource_exists_namespaced,
};

/// List all resources of a kind across all namespaces
pub async fn list_all_namespaces(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let config_server = state.config_server();
    let data = list_all_resources!(&config_server, kind);
    Ok(Json(ListResponse::success(data)))
}

/// List namespace-scoped resources
pub async fn list_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let config_server = state.config_server();
    let data = list_namespaced_resources!(&config_server, kind, ns);
    Ok(Json(ListResponse::success(data)))
}

/// Get a namespace-scoped resource
pub async fn get_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let config_server = state.config_server();
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

    // In K8s mode, write operations are not supported via Admin API
    if state.is_k8s_mode() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let writer = state.writer();
    let config_server = state.config_server();

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

    // Check if resource already exists in ConfigServer (memory cache)
    let exists = resource_exists_namespaced!(&config_server, kind, ns, name);

    if exists {
        tracing::warn!(
            component = "unified_api",
            event = "resource_already_exists",
            kind = %kind_str,
            namespace = %ns,
            name = %name,
            "Resource already exists in ConfigServer"
        );
        return Err(StatusCode::CONFLICT);
    }

    // Parse, persist to writer only (FileWatcher will update ConfigServer)
    match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            // Note: ConfigServer update will be triggered by FileWatcher
        }
        crate::types::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &service)?;
            let json_content = serde_json::to_string(&service).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &ep)?;
            let json_content = serde_json::to_string(&ep).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::Endpoint => {
            let endpoint: Endpoints = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &endpoint)?;
            let json_content = serde_json::to_string(&endpoint).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &tls)?;

            // Use resource_check to validate EdgionTls before apply
            let ctx = ResourceCheckContext::new(&config_server);
            let check_result = check_edgion_tls(&ctx, &tls);

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

            // Log warnings if any
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

            let json_content = serde_json::to_string(&tls).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &plugins)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &metadata)?;
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &linksys)?;
            let json_content = serde_json::to_string(&linksys).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &secret)?;
            let json_content = serde_json::to_string(&secret).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::Gateway => {
            let gateway: Gateway = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &gateway)?;
            let json_content = serde_json::to_string(&gateway).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    }

    tracing::info!(
        component = "unified_api",
        event = "resource_created",
        kind = %kind_str,
        namespace = %ns,
        name = %name,
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

    // In K8s mode, write operations are not supported via Admin API
    if state.is_k8s_mode() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let writer = state.writer();
    let config_server = state.config_server();

    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Parse, validate, persist to writer only (FileWatcher will update ConfigServer)
    match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &service)?;
            let json_content = serde_json::to_string(&service).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &ep)?;
            let json_content = serde_json::to_string(&ep).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::Endpoint => {
            let endpoint: Endpoints = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &endpoint)?;
            let json_content = serde_json::to_string(&endpoint).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &tls)?;

            // Use resource_check to validate EdgionTls before apply
            let ctx = ResourceCheckContext::new(&config_server);
            let check_result = check_edgion_tls(&ctx, &tls);

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

            // Log warnings if any
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

            let json_content = serde_json::to_string(&tls).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &plugins)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &metadata)?;
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &linksys)?;
            let json_content = serde_json::to_string(&linksys).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &secret)?;
            let json_content = serde_json::to_string(&secret).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        crate::types::ResourceKind::Gateway => {
            let gateway: Gateway = parse_resource_and_update_version(&content, true)?;
            validate_resource(&state.schema_validator, kind, &gateway)?;
            let json_content = serde_json::to_string(&gateway).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .set_one(&kind_str, Some(&ns), &name, json_content)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    }

    tracing::info!(
        component = "unified_api",
        event = "resource_updated",
        kind = %kind_str,
        namespace = %ns,
        name = %name,
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

    // In K8s mode, write operations are not supported via Admin API
    if state.is_k8s_mode() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let writer = state.writer();
    let config_server = state.config_server();

    // Check if resource exists in ConfigServer before deleting
    match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let list_data = config_server.routes.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
            // Note: ConfigServer update will be triggered by FileWatcher
        }
        crate::types::ResourceKind::GRPCRoute => {
            let list_data = config_server.grpc_routes.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::TCPRoute => {
            let list_data = config_server.tcp_routes.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::UDPRoute => {
            let list_data = config_server.udp_routes.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::TLSRoute => {
            let list_data = config_server.tls_routes.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::Service => {
            let list_data = config_server.services.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::EndpointSlice => {
            let list_data = config_server.endpoint_slices.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::Endpoint => {
            let list_data = config_server.endpoints.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::EdgionTls => {
            let list_data = config_server.edgion_tls.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let list_data = config_server.edgion_plugins.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::PluginMetaData => {
            let list_data = config_server.plugin_metadata.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::LinkSys => {
            let list_data = config_server.link_sys.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::Secret => {
            let list_data = config_server.secrets.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        crate::types::ResourceKind::Gateway => {
            let list_data = config_server.gateways.list();
            list_data
                .data
                .iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = writer.delete_one(&kind_str, Some(&ns), &name).await;
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    }

    tracing::info!(
        component = "unified_api",
        event = "resource_deleted",
        kind = %kind_str,
        namespace = %ns,
        name = %name,
        "Resource deleted successfully"
    );

    Ok(Json(ApiResponse::success(format!("{} deleted", kind_str))))
}
