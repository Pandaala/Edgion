use axum::{
    extract::{Query, State},
    response::Json,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::core::conf_sync::ConfigServer;
use crate::types::prelude_resources::*;
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::ResourceExt;

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
struct ListResponse<T> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Vec<T>>,
    count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T> ListResponse<T> {
    fn success(data: Vec<T>) -> Self {
        let count = data.len();
        Self {
            success: true,
            data: Some(data),
            count,
            error: None,
        }
    }

    #[allow(dead_code)]
    fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            count: 0,
            error: Some(message),
        }
    }
}

/// Helper function to build resource key from namespace and name
fn build_key(namespace: Option<&String>, name: Option<&String>) -> Result<String, String> {
    match (namespace, name) {
        (Some(ns), Some(n)) => Ok(format!("{}/{}", ns, n)),
        (None, Some(n)) => Ok(n.clone()),
        _ => Err("Missing required parameter: name".to_string()),
    }
}

/// Health check endpoint
async fn health_check() -> Json<ApiResponse<String>> {
    Json(ApiResponse::success("OK".to_string()))
}

/// Get HTTPRoute by namespace and name
async fn get_httproute(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<HTTPRoute>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.routes.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(route) => Json(ApiResponse::success(route)),
        None => Json(ApiResponse::error(format!("HTTPRoute not found: {}", key))),
    }
}

/// List all HTTPRoute resources
async fn list_httproute(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<HTTPRoute>> {
    let list_data = server.routes.list();
    Json(ListResponse::success(list_data.data))
}

/// Get GRPCRoute by namespace and name
async fn get_grpcroute(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<GRPCRoute>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.grpc_routes.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(route) => Json(ApiResponse::success(route)),
        None => Json(ApiResponse::error(format!("GRPCRoute not found: {}", key))),
    }
}

/// List all GRPCRoute resources
async fn list_grpcroute(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<GRPCRoute>> {
    let list_data = server.grpc_routes.list();
    Json(ListResponse::success(list_data.data))
}

/// Get TCPRoute by namespace and name
async fn get_tcproute(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<TCPRoute>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.tcp_routes.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(route) => Json(ApiResponse::success(route)),
        None => Json(ApiResponse::error(format!("TCPRoute not found: {}", key))),
    }
}

/// List all TCPRoute resources
async fn list_tcproute(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<TCPRoute>> {
    let list_data = server.tcp_routes.list();
    Json(ListResponse::success(list_data.data))
}

/// Get UDPRoute by namespace and name
async fn get_udproute(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<UDPRoute>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.udp_routes.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(route) => Json(ApiResponse::success(route)),
        None => Json(ApiResponse::error(format!("UDPRoute not found: {}", key))),
    }
}

/// List all UDPRoute resources
async fn list_udproute(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<UDPRoute>> {
    let list_data = server.udp_routes.list();
    Json(ListResponse::success(list_data.data))
}

/// Get TLSRoute by namespace and name
async fn get_tlsroute(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<TLSRoute>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.tls_routes.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(route) => Json(ApiResponse::success(route)),
        None => Json(ApiResponse::error(format!("TLSRoute not found: {}", key))),
    }
}

/// List all TLSRoute resources
async fn list_tlsroute(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<TLSRoute>> {
    let list_data = server.tls_routes.list();
    Json(ListResponse::success(list_data.data))
}

/// Get Service by namespace and name
async fn get_service(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<Service>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.services.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(service) => Json(ApiResponse::success(service)),
        None => Json(ApiResponse::error(format!("Service not found: {}", key))),
    }
}

/// List all Service resources
async fn list_service(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<Service>> {
    let list_data = server.services.list();
    Json(ListResponse::success(list_data.data))
}

/// Get EndpointSlice by namespace and name
async fn get_endpointslice(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<EndpointSlice>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.endpoint_slices.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(ep) => Json(ApiResponse::success(ep)),
        None => Json(ApiResponse::error(format!("EndpointSlice not found: {}", key))),
    }
}

/// List all EndpointSlice resources
async fn list_endpointslice(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<EndpointSlice>> {
    let list_data = server.endpoint_slices.list();
    Json(ListResponse::success(list_data.data))
}

/// Get EdgionTls by namespace and name
async fn get_edgiontls(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<EdgionTls>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.edgion_tls.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(tls) => Json(ApiResponse::success(tls)),
        None => Json(ApiResponse::error(format!("EdgionTls not found: {}", key))),
    }
}

/// List all EdgionTls resources
async fn list_edgiontls(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<EdgionTls>> {
    let list_data = server.edgion_tls.list();
    Json(ListResponse::success(list_data.data))
}

/// Get EdgionPlugins by namespace and name
async fn get_edgionplugins(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<EdgionPlugins>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.edgion_plugins.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(plugins) => Json(ApiResponse::success(plugins)),
        None => Json(ApiResponse::error(format!("EdgionPlugins not found: {}", key))),
    }
}

/// List all EdgionPlugins resources
async fn list_edgionplugins(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<EdgionPlugins>> {
    let list_data = server.edgion_plugins.list();
    Json(ListResponse::success(list_data.data))
}

/// Get PluginMetaData by namespace and name
async fn get_pluginmetadata(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<PluginMetaData>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.plugin_metadata.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(metadata) => Json(ApiResponse::success(metadata)),
        None => Json(ApiResponse::error(format!("PluginMetaData not found: {}", key))),
    }
}

/// List all PluginMetaData resources
async fn list_pluginmetadata(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<PluginMetaData>> {
    let list_data = server.plugin_metadata.list();
    Json(ListResponse::success(list_data.data))
}

/// Get LinkSys by namespace and name
async fn get_linksys(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<LinkSys>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.link_sys.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(linksys) => Json(ApiResponse::success(linksys)),
        None => Json(ApiResponse::error(format!("LinkSys not found: {}", key))),
    }
}

/// List all LinkSys resources
async fn list_linksys(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<LinkSys>> {
    let list_data = server.link_sys.list();
    Json(ListResponse::success(list_data.data))
}

/// Get Secret by namespace and name
async fn get_secret(
    State(server): State<Arc<ConfigServer>>,
    Query(query): Query<ResourceQuery>,
) -> Json<ApiResponse<Secret>> {
    let key = match build_key(query.namespace.as_ref(), query.name.as_ref()) {
        Ok(k) => k,
        Err(e) => return Json(ApiResponse::error(e)),
    };

    let list_data = server.secrets.list();
    let name = query.name.as_ref().unwrap().as_str();
    let namespace = query.namespace.as_ref().map(|s| s.as_str());
    
    match list_data.data.into_iter().find(|r| {
        r.name_any() == name && r.namespace().as_deref() == namespace
    }) {
        Some(secret) => Json(ApiResponse::success(secret)),
        None => Json(ApiResponse::error(format!("Secret not found: {}", key))),
    }
}

/// List all Secret resources
async fn list_secret(
    State(server): State<Arc<ConfigServer>>,
) -> Json<ListResponse<Secret>> {
    let list_data = server.secrets.list();
    Json(ListResponse::success(list_data.data))
}

/// Create the admin API router with all endpoints
pub fn create_admin_router(config_server: Arc<ConfigServer>) -> Router {
    Router::new()
        // Health check
        .route("/health", get(health_check))
        // HTTPRoute
        .route("/configserver/httproute", get(get_httproute))
        .route("/configserver/httproute/list", get(list_httproute))
        // GRPCRoute
        .route("/configserver/grpcroute", get(get_grpcroute))
        .route("/configserver/grpcroute/list", get(list_grpcroute))
        // TCPRoute
        .route("/configserver/tcproute", get(get_tcproute))
        .route("/configserver/tcproute/list", get(list_tcproute))
        // UDPRoute
        .route("/configserver/udproute", get(get_udproute))
        .route("/configserver/udproute/list", get(list_udproute))
        // TLSRoute
        .route("/configserver/tlsroute", get(get_tlsroute))
        .route("/configserver/tlsroute/list", get(list_tlsroute))
        // Service
        .route("/configserver/service", get(get_service))
        .route("/configserver/service/list", get(list_service))
        // EndpointSlice
        .route("/configserver/endpointslice", get(get_endpointslice))
        .route("/configserver/endpointslice/list", get(list_endpointslice))
        // EdgionTls
        .route("/configserver/edgiontls", get(get_edgiontls))
        .route("/configserver/edgiontls/list", get(list_edgiontls))
        // EdgionPlugins
        .route("/configserver/edgionplugins", get(get_edgionplugins))
        .route("/configserver/edgionplugins/list", get(list_edgionplugins))
        // PluginMetaData
        .route("/configserver/pluginmetadata", get(get_pluginmetadata))
        .route("/configserver/pluginmetadata/list", get(list_pluginmetadata))
        // LinkSys
        .route("/configserver/linksys", get(get_linksys))
        .route("/configserver/linksys/list", get(list_linksys))
        // Secret
        .route("/configserver/secret", get(get_secret))
        .route("/configserver/secret/list", get(list_secret))
        .with_state(config_server)
}

/// Serve the admin API on the specified port
pub async fn serve(config_server: Arc<ConfigServer>, port: u16) -> anyhow::Result<()> {
    let app = create_admin_router(config_server);
    let addr_str = format!("0.0.0.0:{}", port);
    let addr: std::net::SocketAddr = addr_str.parse()?;
    
    tracing::info!(
        component = "admin_api_controller",
        event = "server_starting",
        addr = %addr,
        "Controller Admin API server listening"
    );
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}

