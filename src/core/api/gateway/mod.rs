use crate::core::conf_sync::ConfigClient;
use crate::types::ResourceKind;
use axum::{
    extract::{Path, Query, State},
    response::Json,
    routing::get,
    Router,
};
use kube::ResourceExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Query parameters for resource lookup
#[derive(Deserialize)]
struct ResourceQuery {
    namespace: Option<String>,
    name: Option<String>,
}

/// Standard API response format
#[derive(Serialize)]
struct ApiResponse<T> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T> ApiResponse<T> {
    fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

/// List response format
#[derive(Serialize)]
struct ListResponse {
    success: bool,
    data: Vec<serde_json::Value>,
    count: usize,
}

impl ListResponse {
    fn success(data: Vec<serde_json::Value>) -> Self {
        let count = data.len();
        Self {
            success: true,
            data,
            count,
        }
    }
}

/// Health check endpoint (liveness)
async fn health_check() -> Json<ApiResponse<String>> {
    Json(ApiResponse::success("OK".to_string()))
}

/// Readiness check endpoint - returns OK only when all caches are ready
async fn readiness_check(State(client): State<Arc<ConfigClient>>) -> axum::http::StatusCode {
    match client.is_ready() {
        Ok(()) => axum::http::StatusCode::OK,
        Err(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Helper macro to list all resources from ConfigClient
macro_rules! list_client_resources {
    ($client:expr, $kind:expr) => {{
        match $kind {
            ResourceKind::HTTPRoute => to_json_vec($client.routes().list().data),
            ResourceKind::GRPCRoute => to_json_vec($client.grpc_routes().list().data),
            ResourceKind::TCPRoute => to_json_vec($client.tcp_routes().list().data),
            ResourceKind::UDPRoute => to_json_vec($client.udp_routes().list().data),
            ResourceKind::TLSRoute => to_json_vec($client.tls_routes().list().data),
            ResourceKind::Service => to_json_vec($client.services().list().data),
            ResourceKind::EndpointSlice => to_json_vec($client.endpoint_slices().list().data),
            ResourceKind::Endpoint => to_json_vec($client.endpoints().list().data),
            ResourceKind::EdgionTls => to_json_vec($client.edgion_tls().list().data),
            ResourceKind::EdgionPlugins => to_json_vec($client.edgion_plugins().list().data),
            ResourceKind::EdgionStreamPlugins => to_json_vec($client.edgion_stream_plugins().list().data),
            ResourceKind::ReferenceGrant => to_json_vec($client.reference_grants().list().data),
            ResourceKind::BackendTLSPolicy => to_json_vec($client.backend_tls_policies().list().data),
            ResourceKind::PluginMetaData => to_json_vec($client.plugin_metadata().list().data),
            ResourceKind::LinkSys => to_json_vec($client.link_sys().list().data),
            ResourceKind::GatewayClass => to_json_vec($client.gateway_classes().list().data),
            ResourceKind::Gateway => to_json_vec($client.gateways().list().data),
            ResourceKind::EdgionGatewayConfig => to_json_vec($client.edgion_gateway_configs().list().data),
            _ => vec![],
        }
    }};
}

/// Helper macro to get a single resource from ConfigClient
macro_rules! get_client_resource {
    ($client:expr, $kind:expr, $ns:expr, $name:expr) => {{
        let ns_opt = $ns.as_deref();
        let name_str = $name.as_str();
        match $kind {
            ResourceKind::HTTPRoute => find_resource($client.routes().list().data, ns_opt, name_str),
            ResourceKind::GRPCRoute => find_resource($client.grpc_routes().list().data, ns_opt, name_str),
            ResourceKind::TCPRoute => find_resource($client.tcp_routes().list().data, ns_opt, name_str),
            ResourceKind::UDPRoute => find_resource($client.udp_routes().list().data, ns_opt, name_str),
            ResourceKind::TLSRoute => find_resource($client.tls_routes().list().data, ns_opt, name_str),
            ResourceKind::Service => find_resource($client.services().list().data, ns_opt, name_str),
            ResourceKind::EndpointSlice => find_resource($client.endpoint_slices().list().data, ns_opt, name_str),
            ResourceKind::Endpoint => find_resource($client.endpoints().list().data, ns_opt, name_str),
            ResourceKind::EdgionTls => find_resource($client.edgion_tls().list().data, ns_opt, name_str),
            ResourceKind::EdgionPlugins => find_resource_alt($client.edgion_plugins().list().data, ns_opt, name_str),
            ResourceKind::EdgionStreamPlugins => {
                find_resource_alt($client.edgion_stream_plugins().list().data, ns_opt, name_str)
            }
            ResourceKind::ReferenceGrant => find_resource_alt($client.reference_grants().list().data, ns_opt, name_str),
            ResourceKind::BackendTLSPolicy => {
                find_resource_alt($client.backend_tls_policies().list().data, ns_opt, name_str)
            }
            ResourceKind::PluginMetaData => find_resource($client.plugin_metadata().list().data, ns_opt, name_str),
            ResourceKind::LinkSys => find_resource($client.link_sys().list().data, ns_opt, name_str),
            ResourceKind::GatewayClass => find_cluster_resource($client.gateway_classes().list().data, name_str),
            ResourceKind::Gateway => find_resource($client.gateways().list().data, ns_opt, name_str),
            ResourceKind::EdgionGatewayConfig => {
                find_cluster_resource($client.edgion_gateway_configs().list().data, name_str)
            }
            _ => None,
        }
    }};
}

/// Convert a list of resources to JSON values
fn to_json_vec<T: serde::Serialize>(items: Vec<T>) -> Vec<serde_json::Value> {
    items
        .into_iter()
        .filter_map(|item| serde_json::to_value(item).ok())
        .collect()
}

/// Find a namespaced resource by namespace and name (returns Option<&str> for namespace)
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

/// Find a namespaced resource (alternative namespace() return type - returns Option<String>)
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
fn find_cluster_resource<T: ResourceExt + serde::Serialize>(items: Vec<T>, name: &str) -> Option<serde_json::Value> {
    items
        .into_iter()
        .find(|r| r.name_any() == name)
        .and_then(|r| serde_json::to_value(r).ok())
}

/// Dynamic list endpoint - list all resources of a kind
async fn list_resources(State(client): State<Arc<ConfigClient>>, Path(kind_str): Path<String>) -> Json<ListResponse> {
    let kind = ResourceKind::from_kind_name(&kind_str).unwrap_or(ResourceKind::Unspecified);
    let data = list_client_resources!(&client, kind);
    Json(ListResponse::success(data))
}

/// Dynamic get endpoint - get a single resource by namespace and name
async fn get_resource(
    State(client): State<Arc<ConfigClient>>,
    Path(kind_str): Path<String>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<serde_json::Value>> {
    let Some(name) = query.name else {
        return Json(ApiResponse::error("Missing required parameter: name".to_string()));
    };

    let kind = ResourceKind::from_kind_name(&kind_str).unwrap_or(ResourceKind::Unspecified);
    let resource = get_client_resource!(&client, kind, query.namespace, name);

    match resource {
        Some(r) => Json(ApiResponse::success(r)),
        None => Json(ApiResponse::error(format!("{} not found", kind_str))),
    }
}

/// Create the admin API router with dynamic endpoints
pub fn create_admin_router(config_client: Arc<ConfigClient>) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/ready", get(readiness_check))
        // Dynamic endpoints for all resource types
        .route("/configclient/{kind}", get(get_resource))
        .route("/configclient/{kind}/list", get(list_resources))
        .with_state(config_client)
}

/// Serve the admin API on the specified port
pub async fn serve(config_client: Arc<ConfigClient>, port: u16) -> anyhow::Result<()> {
    let app = create_admin_router(config_client);
    let addr_str = format!("0.0.0.0:{}", port);
    let addr: std::net::SocketAddr = addr_str.parse()?;

    tracing::info!(
        component = "admin_api_gateway",
        event = "server_starting",
        addr = %addr,
        "Gateway Admin API server listening"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
