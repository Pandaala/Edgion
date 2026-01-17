mod cluster_handlers;
mod common;
mod namespaced_handlers;
mod types;

use crate::core::conf_mgr::{load_all_resources, ConfCenter, SchemaValidator};
use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{delete, get, post, put},
    Router,
};
use serde::Serialize;
use std::sync::Arc;
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
    let writer = state.conf_center.writer();
    let config_server = state.conf_center.config_server();

    load_all_resources(writer, config_server)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!(
        component = "unified_api",
        event = "resources_reloaded",
        "All resources reloaded from storage"
    );

    Ok(Json(types::ApiResponse::success(
        "Resources reloaded successfully".to_string(),
    )))
}

// ============= Router Setup =============

/// Create the admin API router with unified K8s-style endpoints
pub fn create_admin_router(conf_center: Arc<ConfCenter>, schema_validator: Arc<SchemaValidator>) -> Router {
    let admin_state = Arc::new(AdminState {
        conf_center,
        schema_validator,
    });

    Router::new()
        // Health check
        .route("/health", get(health_check))
        // Cross-namespace query - List all resources of a kind
        .route(
            "/api/v1/namespaced/{kind}",
            get(namespaced_handlers::list_all_namespaces),
        )
        // Cluster-scoped resources
        .route("/api/v1/cluster/{kind}", get(cluster_handlers::list_cluster))
        .route("/api/v1/cluster/{kind}", post(cluster_handlers::create_cluster))
        .route("/api/v1/cluster/{kind}/{name}", get(cluster_handlers::get_cluster))
        .route("/api/v1/cluster/{kind}/{name}", put(cluster_handlers::update_cluster))
        .route(
            "/api/v1/cluster/{kind}/{name}",
            delete(cluster_handlers::delete_cluster),
        )
        // Namespace-scoped resources
        .route(
            "/api/v1/namespaced/{kind}/{namespace}",
            get(namespaced_handlers::list_namespaced),
        )
        .route(
            "/api/v1/namespaced/{kind}/{namespace}",
            post(namespaced_handlers::create_namespaced),
        )
        .route(
            "/api/v1/namespaced/{kind}/{namespace}/{name}",
            get(namespaced_handlers::get_namespaced),
        )
        .route(
            "/api/v1/namespaced/{kind}/{namespace}/{name}",
            put(namespaced_handlers::update_namespaced),
        )
        .route(
            "/api/v1/namespaced/{kind}/{namespace}/{name}",
            delete(namespaced_handlers::delete_namespaced),
        )
        // Special operations
        .route("/api/v1/reload", post(reload_all_resources))
        .with_state(admin_state)
}

/// Serve the admin API on the specified port
pub async fn serve(
    conf_center: Arc<ConfCenter>,
    schema_validator: Arc<SchemaValidator>,
    port: u16,
) -> anyhow::Result<()> {
    let app = create_admin_router(conf_center, schema_validator);
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
