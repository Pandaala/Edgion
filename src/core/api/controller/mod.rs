mod types;
mod common;
mod cluster_handlers;
mod namespaced_handlers;

use axum::{
    extract::State,
    response::Json,
    routing::{get, post, put, delete},
    Router,
    http::StatusCode,
};
use serde::Serialize;
use std::sync::Arc;
use crate::core::conf_sync::ConfigServer;
use crate::core::conf_mgr::{ResourceMgrAPI, SchemaValidator, load_all_resources_from_store};
use types::AdminState;

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
}

// ============= Legacy Endpoints =============

/// Health check endpoint
async fn health_check() -> Json<ApiResponse<String>> {
    Json(ApiResponse::success("OK".to_string()))
}

/// Reload all resources from storage
async fn reload_all_resources(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<types::ApiResponse<String>>, StatusCode> {
    let resource_mgr = state.resource_mgr.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let store = resource_mgr
        .get_backend(None)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    load_all_resources_from_store(store, state.config_server.clone())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!(
        component = "unified_api",
        event = "resources_reloaded",
        "All resources reloaded from storage"
    );

    Ok(Json(types::ApiResponse::success("Resources reloaded successfully".to_string())))
}

// ============= Router Setup =============

/// Create the admin API router with unified K8s-style endpoints
pub fn create_admin_router(
    config_server: Arc<ConfigServer>,
    resource_mgr: Option<Arc<ResourceMgrAPI>>,
    schema_validator: Arc<SchemaValidator>,
) -> Router {
    let admin_state = Arc::new(AdminState {
        config_server,
        resource_mgr,
        schema_validator,
    });

    Router::new()
        // Health check
        .route("/health", get(health_check))

        // Cross-namespace query - List all resources of a kind
        .route("/api/v1/namespaced/{kind}", get(namespaced_handlers::list_all_namespaces))

        // Cluster-scoped resources
        .route("/api/v1/cluster/{kind}", get(cluster_handlers::list_cluster))
        .route("/api/v1/cluster/{kind}", post(cluster_handlers::create_cluster))
        .route("/api/v1/cluster/{kind}/{name}", get(cluster_handlers::get_cluster))
        .route("/api/v1/cluster/{kind}/{name}", put(cluster_handlers::update_cluster))
        .route("/api/v1/cluster/{kind}/{name}", delete(cluster_handlers::delete_cluster))

        // Namespace-scoped resources
        .route("/api/v1/namespaced/{kind}/{namespace}", get(namespaced_handlers::list_namespaced))
        .route("/api/v1/namespaced/{kind}/{namespace}", post(namespaced_handlers::create_namespaced))
        .route("/api/v1/namespaced/{kind}/{namespace}/{name}", get(namespaced_handlers::get_namespaced))
        .route("/api/v1/namespaced/{kind}/{namespace}/{name}", put(namespaced_handlers::update_namespaced))
        .route("/api/v1/namespaced/{kind}/{namespace}/{name}", delete(namespaced_handlers::delete_namespaced))

        // Special operations
        .route("/api/v1/reload", post(reload_all_resources))
        .with_state(admin_state)
}

/// Serve the admin API on the specified port
pub async fn serve(
    config_server: Arc<ConfigServer>,
    resource_mgr: Option<Arc<ResourceMgrAPI>>,
    schema_validator: Arc<SchemaValidator>,
    port: u16,
) -> anyhow::Result<()> {
    let app = create_admin_router(config_server, resource_mgr, schema_validator);
    let addr_str = format!("0.0.0.0:{}", port);
    let addr: std::net::SocketAddr = addr_str.parse()?;

    tracing::info!(
        component = "unified_api",
        event = "server_starting",
        addr = %addr,
        "Controller Admin API server listening with unified K8s-style routes"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}