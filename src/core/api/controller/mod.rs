mod cluster_handlers;
mod common;
mod configserver_handlers;
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

/// Health check endpoint - always returns OK if server is up
async fn health_check() -> Json<ApiResponse<String>> {
    Json(ApiResponse::success("OK".to_string()))
}

/// Readiness check endpoint - returns OK only when ConfigServer is ready
async fn readiness_check(State(state): State<Arc<AdminState>>) -> Result<Json<ApiResponse<String>>, StatusCode> {
    if state.is_ready() {
        Ok(Json(ApiResponse::success("Ready".to_string())))
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

/// Reload all resources from storage
async fn reload_all_resources(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<types::ApiResponse<String>>, StatusCode> {
    let writer = state.conf_center.writer();
    let config_server = state.config_server()?;

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
        // Health check (liveness)
        .route("/health", get(health_check))
        // Readiness check (ready to serve traffic - ConfigServer ready)
        .route("/ready", get(readiness_check))
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
        // ConfigServer endpoints (for edgion-ctl --target server)
        .route("/configserver/{kind}/list", get(configserver_handlers::list_resources))
        .route("/configserver/{kind}", get(configserver_handlers::get_resource))
        .with_state(admin_state)
}

/// Serve the admin API on the specified address
pub async fn serve(
    conf_center: Arc<ConfCenter>,
    schema_validator: Arc<SchemaValidator>,
    addr: std::net::SocketAddr,
) -> anyhow::Result<()> {
    let app = create_admin_router(conf_center, schema_validator);

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

/// Serve the admin API with graceful shutdown support
pub async fn serve_with_shutdown(
    conf_center: Arc<ConfCenter>,
    schema_validator: Arc<SchemaValidator>,
    addr: std::net::SocketAddr,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let app = create_admin_router(conf_center, schema_validator);

    tracing::info!(
        component = "admin_api",
        addr = %addr,
        "Starting Admin API server with graceful shutdown support"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    tracing::info!(component = "admin_api", "Admin API server stopped");
    Ok(())
}
