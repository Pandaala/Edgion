use axum::{
    extract::{Path, State},
    response::Json,
    body::Bytes,
    http::StatusCode,
};
use std::sync::Arc;
use crate::core::conf_sync::CacheEventDispatch;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::utils::extract_resource_metadata;
use crate::types::{ResourceKind, prelude_resources::*};
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::ResourceExt;

use super::types::*;

/// Helper function to validate a resource against its schema
fn validate_resource<T: serde::Serialize>(
    validator: &crate::core::conf_mgr::SchemaValidator,
    kind: ResourceKind,
    resource: &T,
) -> Result<(), StatusCode> {
    let json_value = serde_json::to_value(resource)
        .map_err(|e| {
            tracing::warn!(
                component = "unified_api",
                error = %e,
                "Failed to convert resource to JSON for validation"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    validator.validate(kind, &json_value)
        .map_err(|e| {
            tracing::warn!(
                component = "unified_api",
                kind = ?kind,
                error = %e,
                "Schema validation failed"
            );
            StatusCode::BAD_REQUEST
        })?;
    
    Ok(())
}

/// Parse request body as either JSON or YAML
/// Tries JSON first, falls back to YAML if JSON parsing fails
fn parse_resource<T>(body: &str) -> Result<T, StatusCode>
where
    T: serde::de::DeserializeOwned,
{
    // Try JSON first (more common in API calls)
    if let Ok(resource) = serde_json::from_str::<T>(body) {
        return Ok(resource);
    }
    
    // Fall back to YAML
    serde_yaml::from_str::<T>(body).map_err(|e| {
        tracing::warn!(
            component = "unified_api",
            error = %e,
            "Failed to parse request body as JSON or YAML"
        );
        StatusCode::BAD_REQUEST
    })
}

/// Parse ResourceKind from string (case-insensitive)
fn parse_kind(kind_str: &str) -> Result<ResourceKind, String> {
    ResourceKind::from_kind_name(kind_str)
        .ok_or_else(|| format!("Unknown resource kind: {}", kind_str))
}

/// Determine if a resource kind is cluster-scoped
fn is_cluster_scoped(kind: &ResourceKind) -> bool {
    matches!(kind, ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig)
}
// ============= Cross-namespace Query =============

/// Helper macro to convert list data to JSON Value Vec
macro_rules! list_to_json {
    ($list_data:expr) => {{
        $list_data
            .into_iter()
            .map(|item| serde_json::to_value(item).unwrap_or(serde_json::Value::Null))
            .collect::<Vec<_>>()
    }};
}

/// List all resources of a kind across all namespaces
pub async fn list_all_namespaces(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let data = match kind {
        ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            list_to_json!(list_data.data)
        }
        ResourceKind::Secret => {
            let list_data = state.config_server.secrets.list();
            list_to_json!(list_data.data)
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    };
    
    Ok(Json(ListResponse::success(data)))
}

// ============= Cluster-scoped Resources =============

/// List all cluster-scoped resources of a kind
pub async fn list_cluster(
    State(state): State<Arc<AdminState>>,
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
    State(state): State<Arc<AdminState>>,
    Path((kind_str, name)): Path<(String, String)>,
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
    
    let metadata = extract_resource_metadata(&content).ok_or(StatusCode::BAD_REQUEST)?;
    let name = metadata.name.ok_or(StatusCode::BAD_REQUEST)?;
    
    // Parse, validate, and persist
    match kind {
        ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &gc)?;
            let json_content = serde_json::to_string(&gc)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, None, &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        ResourceKind::EdgionGatewayConfig => {
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
        ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &gc)?;
            let json_content = serde_json::to_string(&gc)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, None, &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        ResourceKind::EdgionGatewayConfig => {
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
        ResourceKind::GatewayClass => {
            let _: GatewayClass = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, None, &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        ResourceKind::EdgionGatewayConfig => {
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

// ============= Namespace-scoped Resources =============

/// List namespace-scoped resources
pub async fn list_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((ns, kind_str)): Path<(String, String)>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let data = match kind {
        ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            list_to_json!(filtered)
        }
        ResourceKind::Secret => {
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
    Path((ns, kind_str, name)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let resource = match kind {
        ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()))
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ResourceKind::Secret => {
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
    Path((ns, kind_str)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let metadata = extract_resource_metadata(&content).ok_or(StatusCode::BAD_REQUEST)?;
    let name = metadata.name.ok_or(StatusCode::BAD_REQUEST)?;
    
    // Parse, persist, and update cache in one step
    match kind {
        ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.grpc_routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tcp_routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.udp_routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tls_routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::Service => {
            let service: Service = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &service)?;
            let json_content = serde_json::to_string(&service)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.services.apply_change(ResourceChange::EventAdd, service);
        }
        ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &ep)?;
            let json_content = serde_json::to_string(&ep)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.endpoint_slices.apply_change(ResourceChange::EventAdd, ep);
        }
        ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &tls)?;
            let json_content = serde_json::to_string(&tls)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_tls.apply_change(ResourceChange::EventAdd, tls);
        }
        ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &plugins)?;
            let json_content = serde_json::to_string(&plugins)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_plugins.apply_change(ResourceChange::EventAdd, plugins);
        }
        ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &metadata)?;
            let json_content = serde_json::to_string(&metadata)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.plugin_metadata.apply_change(ResourceChange::EventAdd, metadata);
        }
        ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &linksys)?;
            let json_content = serde_json::to_string(&linksys)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.link_sys.apply_change(ResourceChange::EventAdd, linksys);
        }
        ResourceKind::Secret => {
            let secret: Secret = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &secret)?;
            let json_content = serde_json::to_string(&secret)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.secrets.apply_change(ResourceChange::EventAdd, secret);
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
    Path((ns, kind_str, name)): Path<(String, String, String)>,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    // Parse, validate, persist, and update cache in one step
    match kind {
        ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.grpc_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tcp_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.udp_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &route)?;
            let json_content = serde_json::to_string(&route)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tls_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::Service => {
            let service: Service = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &service)?;
            let json_content = serde_json::to_string(&service)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.services.apply_change(ResourceChange::EventUpdate, service);
        }
        ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &ep)?;
            let json_content = serde_json::to_string(&ep)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.endpoint_slices.apply_change(ResourceChange::EventUpdate, ep);
        }
        ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &tls)?;
            let json_content = serde_json::to_string(&tls)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_tls.apply_change(ResourceChange::EventUpdate, tls);
        }
        ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &plugins)?;
            let json_content = serde_json::to_string(&plugins)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_plugins.apply_change(ResourceChange::EventUpdate, plugins);
        }
        ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &metadata)?;
            let json_content = serde_json::to_string(&metadata)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.plugin_metadata.apply_change(ResourceChange::EventUpdate, metadata);
        }
        ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &linksys)?;
            let json_content = serde_json::to_string(&linksys)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.link_sys.apply_change(ResourceChange::EventUpdate, linksys);
        }
        ResourceKind::Secret => {
            let secret: Secret = parse_resource(&content)?;
            validate_resource(&state.schema_validator, kind, &secret)?;
            let json_content = serde_json::to_string(&secret)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            resource_mgr.set_one(&kind_str, Some(&ns), &name, json_content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.secrets.apply_change(ResourceChange::EventUpdate, secret);
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
    Path((ns, kind_str, name)): Path<(String, String, String)>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    
    // Read, validate, delete, and remove from cache in one step
    let content = resource_mgr
        .get_one(&kind_str, Some(&ns), &name)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    
    match kind {
        ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.grpc_routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tcp_routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.udp_routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tls_routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::Service => {
            let service: Service = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.services.apply_change(ResourceChange::EventDelete, service);
        }
        ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.endpoint_slices.apply_change(ResourceChange::EventDelete, ep);
        }
        ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_tls.apply_change(ResourceChange::EventDelete, tls);
        }
        ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_plugins.apply_change(ResourceChange::EventDelete, plugins);
        }
        ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.plugin_metadata.apply_change(ResourceChange::EventDelete, metadata);
        }
        ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.link_sys.apply_change(ResourceChange::EventDelete, linksys);
        }
        ResourceKind::Secret => {
            let secret: Secret = parse_resource(&content)?;
            resource_mgr.delete_one(&kind_str, Some(&ns), &name).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.secrets.apply_change(ResourceChange::EventDelete, secret);
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

