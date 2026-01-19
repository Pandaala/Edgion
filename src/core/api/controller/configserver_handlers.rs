//! ConfigServer handlers for edgion-ctl `--target server` support
//!
//! These handlers provide read-only access to ConfigServer cache data,
//! with response format compatible with Gateway's `/configclient/` API.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use kube::ResourceExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::common::parse_kind;
use super::types::AdminState;
use crate::{list_all_resources, list_to_json};

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

/// Helper macro to get a single resource from ConfigServer by namespace and name
macro_rules! get_server_resource {
    ($server:expr, $kind:expr, $ns:expr, $name:expr) => {{
        use $crate::types::ResourceKind;
        let ns_opt = $ns.as_deref();
        let name_str = $name.as_str();
        match $kind {
            ResourceKind::HTTPRoute => find_resource($server.routes.list().data, ns_opt, name_str),
            ResourceKind::GRPCRoute => find_resource($server.grpc_routes.list().data, ns_opt, name_str),
            ResourceKind::TCPRoute => find_resource($server.tcp_routes.list().data, ns_opt, name_str),
            ResourceKind::UDPRoute => find_resource($server.udp_routes.list().data, ns_opt, name_str),
            ResourceKind::TLSRoute => find_resource($server.tls_routes.list().data, ns_opt, name_str),
            ResourceKind::Service => find_resource($server.services.list().data, ns_opt, name_str),
            ResourceKind::EndpointSlice => find_resource($server.endpoint_slices.list().data, ns_opt, name_str),
            ResourceKind::Endpoint => find_resource($server.endpoints.list().data, ns_opt, name_str),
            ResourceKind::EdgionTls => find_resource($server.edgion_tls.list().data, ns_opt, name_str),
            ResourceKind::EdgionPlugins => find_resource_alt($server.edgion_plugins.list().data, ns_opt, name_str),
            ResourceKind::EdgionStreamPlugins => {
                find_resource_alt($server.edgion_stream_plugins.list().data, ns_opt, name_str)
            }
            ResourceKind::ReferenceGrant => find_resource_alt($server.reference_grants.list().data, ns_opt, name_str),
            ResourceKind::BackendTLSPolicy => {
                find_resource_alt($server.backend_tls_policies.list().data, ns_opt, name_str)
            }
            ResourceKind::PluginMetaData => find_resource($server.plugin_metadata.list().data, ns_opt, name_str),
            ResourceKind::LinkSys => find_resource($server.link_sys.list().data, ns_opt, name_str),
            ResourceKind::Secret => find_resource($server.secrets.list().data, ns_opt, name_str),
            ResourceKind::GatewayClass => find_cluster_resource($server.gateway_classes.list().data, name_str),
            ResourceKind::Gateway => find_resource($server.gateways.list().data, ns_opt, name_str),
            ResourceKind::EdgionGatewayConfig => {
                find_cluster_resource($server.edgion_gateway_configs.list().data, name_str)
            }
            ResourceKind::Unspecified => None,
        }
    }};
}

/// Find a namespaced resource by namespace and name
fn find_resource<T: ResourceExt + serde::Serialize>(
    items: Vec<T>,
    namespace: Option<&str>,
    name: &str,
) -> Option<serde_json::Value> {
    items
        .into_iter()
        .find(|r| r.name_any() == name && r.namespace().as_deref() == namespace)
        .and_then(|r| serde_json::to_value(r).ok())
}

/// Find a namespaced resource (alternative namespace() return type)
fn find_resource_alt<T: ResourceExt + serde::Serialize>(
    items: Vec<T>,
    namespace: Option<&str>,
    name: &str,
) -> Option<serde_json::Value> {
    items
        .into_iter()
        .find(|r| r.name_any() == name && r.namespace().as_deref() == namespace)
        .and_then(|r| serde_json::to_value(r).ok())
}

/// Find a cluster-scoped resource by name only
fn find_cluster_resource<T: ResourceExt + serde::Serialize>(
    items: Vec<T>,
    name: &str,
) -> Option<serde_json::Value> {
    items
        .into_iter()
        .find(|r| r.name_any() == name)
        .and_then(|r| serde_json::to_value(r).ok())
}

/// List all resources of a kind from ConfigServer
/// GET /configserver/{kind}/list
pub async fn list_resources(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let config_server = state.config_server()?;
    let data = list_all_resources!(&config_server, kind);
    Ok(Json(ListResponse::success(data)))
}

/// Get a single resource from ConfigServer by namespace and name
/// GET /configserver/{kind}?namespace=xxx&name=xxx
pub async fn get_resource(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<ApiResponse<serde_json::Value>>, StatusCode> {
    let Some(name) = query.name else {
        return Ok(Json(ApiResponse::error(
            "Missing required parameter: name".to_string(),
        )));
    };

    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let config_server = state.config_server()?;
    let resource = get_server_resource!(&config_server, kind, query.namespace, name);

    match resource {
        Some(r) => Ok(Json(ApiResponse::success(r))),
        None => Ok(Json(ApiResponse::error(format!("{} not found", kind_str)))),
    }
}
