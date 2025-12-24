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

/// List all resources of a kind across all namespaces
pub async fn list_all_namespaces(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<String, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let json_result = match kind {
        ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        ResourceKind::Secret => {
            let list_data = state.config_server.secrets.list();
            serde_json::to_string(&ListResponse::success(list_data.data))
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    };
    
    json_result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// ============= Cluster-scoped Resources =============

/// List all cluster-scoped resources of a kind
pub async fn list_cluster(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<String, StatusCode> {
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
) -> Result<String, StatusCode> {
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
    let yaml = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let metadata = extract_resource_metadata(&yaml).ok_or(StatusCode::BAD_REQUEST)?;
    let name = metadata.name.ok_or(StatusCode::BAD_REQUEST)?;
    
    resource_mgr
        .set_one(&kind_str, None, &name, yaml)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
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
    let yaml = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    resource_mgr
        .set_one(&kind_str, None, &name, yaml)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
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
    
    resource_mgr
        .delete_one(&kind_str, None, &name)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
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
) -> Result<String, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let json_result = match kind {
        ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        ResourceKind::Secret => {
            let list_data = state.config_server.secrets.list();
            let filtered: Vec<_> = list_data.data.into_iter()
                .filter(|r| r.namespace().as_deref() == Some(ns.as_str()))
                .collect();
            serde_json::to_string(&ListResponse::success(filtered))
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    };
    
    json_result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Get a namespace-scoped resource
pub async fn get_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((ns, kind_str, name)): Path<(String, String, String)>,
) -> Result<String, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let json_result = match kind {
        ResourceKind::HTTPRoute => {
            let list_data = state.config_server.routes.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::GRPCRoute => {
            let list_data = state.config_server.grpc_routes.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::TCPRoute => {
            let list_data = state.config_server.tcp_routes.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::UDPRoute => {
            let list_data = state.config_server.udp_routes.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::TLSRoute => {
            let list_data = state.config_server.tls_routes.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::Service => {
            let list_data = state.config_server.services.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::EndpointSlice => {
            let list_data = state.config_server.endpoint_slices.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::EdgionTls => {
            let list_data = state.config_server.edgion_tls.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::EdgionPlugins => {
            let list_data = state.config_server.edgion_plugins.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::PluginMetaData => {
            let list_data = state.config_server.plugin_metadata.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::LinkSys => {
            let list_data = state.config_server.link_sys.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        ResourceKind::Secret => {
            let list_data = state.config_server.secrets.list();
            let resource = list_data.data.into_iter()
                .find(|r| r.name_any() == name && r.namespace().as_deref() == Some(ns.as_str()));
            match resource {
                Some(r) => serde_json::to_string(&ApiResponse::success(r)),
                None => return Err(StatusCode::NOT_FOUND),
            }
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    };
    
    json_result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Create a namespace-scoped resource
pub async fn create_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((ns, kind_str)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    
    let yaml = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let metadata = extract_resource_metadata(&yaml).ok_or(StatusCode::BAD_REQUEST)?;
    let name = metadata.name.ok_or(StatusCode::BAD_REQUEST)?;
    
    // Store in ResourceStore
    resource_mgr
        .set_one(&kind_str, Some(&ns), &name, yaml.clone())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    // Update ConfigServer memory
    match kind {
        ResourceKind::HTTPRoute => {
            let route: HTTPRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::GRPCRoute => {
            let route: GRPCRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.grpc_routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::TCPRoute => {
            let route: TCPRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.tcp_routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::UDPRoute => {
            let route: UDPRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.udp_routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::TLSRoute => {
            let route: TLSRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.tls_routes.apply_change(ResourceChange::EventAdd, route);
        }
        ResourceKind::Service => {
            let service: Service = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.services.apply_change(ResourceChange::EventAdd, service);
        }
        ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.endpoint_slices.apply_change(ResourceChange::EventAdd, ep);
        }
        ResourceKind::EdgionTls => {
            let tls: EdgionTls = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.edgion_tls.apply_change(ResourceChange::EventAdd, tls);
        }
        ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.edgion_plugins.apply_change(ResourceChange::EventAdd, plugins);
        }
        ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.plugin_metadata.apply_change(ResourceChange::EventAdd, metadata);
        }
        ResourceKind::LinkSys => {
            let linksys: LinkSys = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.link_sys.apply_change(ResourceChange::EventAdd, linksys);
        }
        ResourceKind::Secret => {
            let secret: Secret = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
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
    
    let yaml = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    // Store in ResourceStore
    resource_mgr
        .set_one(&kind_str, Some(&ns), &name, yaml.clone())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    // Update ConfigServer memory
    match kind {
        ResourceKind::HTTPRoute => {
            let route: HTTPRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::GRPCRoute => {
            let route: GRPCRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.grpc_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::TCPRoute => {
            let route: TCPRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.tcp_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::UDPRoute => {
            let route: UDPRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.udp_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::TLSRoute => {
            let route: TLSRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.tls_routes.apply_change(ResourceChange::EventUpdate, route);
        }
        ResourceKind::Service => {
            let service: Service = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.services.apply_change(ResourceChange::EventUpdate, service);
        }
        ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.endpoint_slices.apply_change(ResourceChange::EventUpdate, ep);
        }
        ResourceKind::EdgionTls => {
            let tls: EdgionTls = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.edgion_tls.apply_change(ResourceChange::EventUpdate, tls);
        }
        ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.edgion_plugins.apply_change(ResourceChange::EventUpdate, plugins);
        }
        ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.plugin_metadata.apply_change(ResourceChange::EventUpdate, metadata);
        }
        ResourceKind::LinkSys => {
            let linksys: LinkSys = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
            state.config_server.link_sys.apply_change(ResourceChange::EventUpdate, linksys);
        }
        ResourceKind::Secret => {
            let secret: Secret = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
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
    
    // First get the resource from storage
    let yaml = resource_mgr
        .get_one(&kind_str, Some(&ns), &name)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    
    // Delete from storage
    resource_mgr
        .delete_one(&kind_str, Some(&ns), &name)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    // Remove from ConfigServer memory
    match kind {
        ResourceKind::HTTPRoute => {
            let route: HTTPRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::GRPCRoute => {
            let route: GRPCRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.grpc_routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::TCPRoute => {
            let route: TCPRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tcp_routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::UDPRoute => {
            let route: UDPRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.udp_routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::TLSRoute => {
            let route: TLSRoute = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.tls_routes.apply_change(ResourceChange::EventDelete, route);
        }
        ResourceKind::Service => {
            let service: Service = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.services.apply_change(ResourceChange::EventDelete, service);
        }
        ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.endpoint_slices.apply_change(ResourceChange::EventDelete, ep);
        }
        ResourceKind::EdgionTls => {
            let tls: EdgionTls = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_tls.apply_change(ResourceChange::EventDelete, tls);
        }
        ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.edgion_plugins.apply_change(ResourceChange::EventDelete, plugins);
        }
        ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.plugin_metadata.apply_change(ResourceChange::EventDelete, metadata);
        }
        ResourceKind::LinkSys => {
            let linksys: LinkSys = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            state.config_server.link_sys.apply_change(ResourceChange::EventDelete, linksys);
        }
        ResourceKind::Secret => {
            let secret: Secret = serde_yaml::from_str(&yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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

