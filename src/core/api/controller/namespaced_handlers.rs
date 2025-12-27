use axum::{
    extract::{Path, State},
    response::Json,
    body::Bytes,
    http::StatusCode,
};
use std::sync::Arc;
use crate::core::conf_sync::CacheEventDispatch;
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::prelude_resources::*;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::ResourceExt;

use super::types::*;
use super::common::*;
use crate::list_to_json;

/// List all resources of a kind across all namespaces
pub async fn list_all_namespaces(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let data = match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::Endpoint => {
            let list_data = state.config_server.endpoints.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::EdgionStreamPlugins => {
            let list_data = state.config_server.edgion_stream_plugins.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::ReferenceGrant => {
            let list_data = state.config_server.reference_grants.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::BackendTLSPolicy => {
            let list_data = state.config_server.backend_tls_policies.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            list_to_json!(list_data.data)
        }
        crate::types::ResourceKind::Secret => {
            let list_data = state.config_server.secrets.list();
            list_to_json!(list_data.data)
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    };
    
    Ok(Json(ListResponse::success(data)))
}

/// List namespace-scoped resources
pub async fn list_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let data = match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::Endpoint => {
            let list_data = state.config_server.endpoints.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::EdgionStreamPlugins => {
            let list_data = state.config_server.edgion_stream_plugins.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::ReferenceGrant => {
            let list_data = state.config_server.reference_grants.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::BackendTLSPolicy => {
            let list_data = state.config_server.backend_tls_policies.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        crate::types::ResourceKind::Secret => {
            let list_data = state.config_server.secrets.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    };
    
    Ok(Json(ListResponse::success(data)))
}

/// Get a namespace-scoped resource
pub async fn get_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let resource = match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::Endpoint => {
            let list_data = state.config_server.endpoints.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::EdgionStreamPlugins => {
            let list_data = state.config_server.edgion_stream_plugins.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::ReferenceGrant => {
            let list_data = state.config_server.reference_grants.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::BackendTLSPolicy => {
            let list_data = state.config_server.backend_tls_policies.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        crate::types::ResourceKind::Secret => {
            let list_data = state.config_server.secrets.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    };
    
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
    
    let resource_mgr = state.resource_mgr.as_ref().ok_or_else(|| {
        tracing::error!("Resource manager not available");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    
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
    let exists = match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::Endpoint => {
            let list_data = state.config_server.endpoints.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        crate::types::ResourceKind::Secret => {
            let list_data = state.config_server.secrets.list();
            list_data.data.iter().any(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    };
    
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
    
    // Parse, persist, and update cache in one step
    match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.routes.apply_change(ResourceChange::EventAdd, route);
        }
        crate::types::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.grpc_routes.apply_change(ResourceChange::EventAdd, route);
        }
        crate::types::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tcp_routes.apply_change(ResourceChange::EventAdd, route);
        }
        crate::types::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.udp_routes.apply_change(ResourceChange::EventAdd, route);
        }
        crate::types::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tls_routes.apply_change(ResourceChange::EventAdd, route);
        }
        crate::types::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &service)?;
            let json_content = serde_json::to_string(&service)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.services.apply_change(ResourceChange::EventAdd, service);
        }
        crate::types::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &ep)?;
            let json_content = serde_json::to_string(&ep)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.endpoint_slices.apply_change(ResourceChange::EventAdd, ep);
        }
        crate::types::ResourceKind::Endpoint => {
            let endpoint: Endpoints = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &endpoint)?;
            let json_content = serde_json::to_string(&endpoint)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.endpoints.apply_change(ResourceChange::EventAdd, endpoint);
        }
        crate::types::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &tls)?;
            let json_content = serde_json::to_string(&tls)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.apply_edgion_tls_change(ResourceChange::EventAdd, tls);
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &plugins)?;
            let json_content = serde_json::to_string(&plugins)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_plugins.apply_change(ResourceChange::EventAdd, plugins);
        }
        crate::types::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &metadata)?;
            let json_content = serde_json::to_string(&metadata)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.plugin_metadata.apply_change(ResourceChange::EventAdd, metadata);
        }
        crate::types::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &linksys)?;
            let json_content = serde_json::to_string(&linksys)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.link_sys.apply_change(ResourceChange::EventAdd, linksys);
        }
        crate::types::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &secret)?;
            let json_content = serde_json::to_string(&secret)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.apply_secret_change(ResourceChange::EventAdd, secret);
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
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    // Parse, validate, persist, and update cache in one step
    match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.routes.apply_change(ResourceChange::EventUpdate, route);
        }
        crate::types::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.grpc_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        crate::types::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tcp_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        crate::types::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.udp_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        crate::types::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tls_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        crate::types::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &service)?;
            let json_content = serde_json::to_string(&service)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.services.apply_change(ResourceChange::EventUpdate, service);
        }
        crate::types::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &ep)?;
            let json_content = serde_json::to_string(&ep)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.endpoint_slices.apply_change(ResourceChange::EventUpdate, ep);
        }
        crate::types::ResourceKind::Endpoint => {
            let endpoint: Endpoints = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &endpoint)?;
            let json_content = serde_json::to_string(&endpoint)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.endpoints.apply_change(ResourceChange::EventUpdate, endpoint);
        }
        crate::types::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &tls)?;
            let json_content = serde_json::to_string(&tls)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.apply_edgion_tls_change(ResourceChange::EventUpdate, tls);
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &plugins)?;
            let json_content = serde_json::to_string(&plugins)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_plugins.apply_change(ResourceChange::EventUpdate, plugins);
        }
        crate::types::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &metadata)?;
            let json_content = serde_json::to_string(&metadata)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.plugin_metadata.apply_change(ResourceChange::EventUpdate, metadata);
        }
        crate::types::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &linksys)?;
            let json_content = serde_json::to_string(&linksys)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.link_sys.apply_change(ResourceChange::EventUpdate, linksys);
        }
        crate::types::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(
                &content,
                state.resource_mgr.is_some()
            )?;
            validate_resource(&state.schema_validator, kind, &secret)?;
            let json_content = serde_json::to_string(&secret)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.apply_secret_change(ResourceChange::EventUpdate, secret);
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
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    
    // Find and delete resource from ConfigServer (memory cache) and persistence
    match kind {
        crate::types::ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            let mut route = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut route); // Update version for EventDelete
            state.config_server.routes.apply_change(ResourceChange::EventDelete, route);
        }
        crate::types::ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            let mut route = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut route);
            state.config_server.grpc_routes.apply_change(ResourceChange::EventDelete, route);
        }
        crate::types::ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            let mut route = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut route);
            state.config_server.tcp_routes.apply_change(ResourceChange::EventDelete, route);
        }
        crate::types::ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            let mut route = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut route);
            state.config_server.udp_routes.apply_change(ResourceChange::EventDelete, route);
        }
        crate::types::ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            let mut route = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut route);
            state.config_server.tls_routes.apply_change(ResourceChange::EventDelete, route);
        }
        crate::types::ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            let mut service = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut service);
            state.config_server.services.apply_change(ResourceChange::EventDelete, service);
        }
        crate::types::ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            let mut ep = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut ep);
            state.config_server.endpoint_slices.apply_change(ResourceChange::EventDelete, ep);
        }
        crate::types::ResourceKind::Endpoint => {
            let list_data = state.config_server.endpoints.list();
            let mut endpoint = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut endpoint);
            state.config_server.endpoints.apply_change(ResourceChange::EventDelete, endpoint);
        }
        crate::types::ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            let mut tls = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut tls);
            state.config_server.apply_edgion_tls_change(ResourceChange::EventDelete, tls);
        }
        crate::types::ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            let mut plugins = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut plugins);
            state.config_server.edgion_plugins.apply_change(ResourceChange::EventDelete, plugins);
        }
        crate::types::ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            let mut metadata = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut metadata);
            state.config_server.plugin_metadata.apply_change(ResourceChange::EventDelete, metadata);
        }
        crate::types::ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            let mut linksys = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut linksys);
            state.config_server.link_sys.apply_change(ResourceChange::EventDelete, linksys);
        }
        crate::types::ResourceKind::Secret => {
            let list_data = state.config_server.secrets.list();
            let mut secret = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .ok_or(StatusCode::NOT_FOUND)?;
            let _ = resource_mgr.delete_one(&kind_str, Some(&ns), &name).await;
            update_resource_version(&mut secret);
            state.config_server.apply_secret_change(ResourceChange::EventDelete, secret);
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

