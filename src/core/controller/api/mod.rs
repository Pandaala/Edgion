mod cluster_handlers;
mod common;
mod configserver_handlers;
mod namespaced_handlers;
mod region_route_handlers;
mod types;

use crate::core::common::api::{self, ApiResponse};
use crate::core::controller::conf_mgr::{ConfMgr, SchemaValidator};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post, put},
    Router,
};
use serde::Serialize;
use std::sync::Arc;
use types::AdminState;

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

/// Server info response
#[derive(Serialize)]
struct ServerInfoResponse {
    server_id: String,
    ready: bool,
    mode: String,
}

/// Get server info including server_id
///
/// This endpoint returns the current server_id which changes on reload.
/// Useful for verifying that reload has taken effect.
async fn get_server_info(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<ApiResponse<ServerInfoResponse>>, StatusCode> {
    match state.config_sync_server() {
        Ok(server) => {
            let info = ServerInfoResponse {
                server_id: server.server_id(),
                ready: true,
                mode: "controller".to_string(),
            };
            Ok(Json(ApiResponse::success(info)))
        }
        Err(_) => {
            // Server not ready yet
            let info = ServerInfoResponse {
                server_id: String::new(),
                ready: false,
                mode: "controller".to_string(),
            };
            Ok(Json(ApiResponse::success(info)))
        }
    }
}

/// Reload all resources - triggers a full restart of the configuration center
///
/// This endpoint triggers a full reload:
/// 1. Stop current controllers
/// 2. Clear PROCESSOR_REGISTRY
/// 3. Create new ConfigSyncServer (new server_id)
/// 4. Restart controllers (full Init -> InitApply -> InitDone flow)
///
/// Gateway clients will detect the server_id change and re-list all resources.
async fn reload_all_resources(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<types::ApiResponse<String>>, StatusCode> {
    tracing::info!(
        component = "admin_api",
        event = "reload_requested",
        "Reload requested via Admin API"
    );

    match state.conf_mgr.request_reload() {
        Ok(()) => {
            tracing::info!(
                component = "admin_api",
                event = "reload_initiated",
                "Reload initiated successfully"
            );
            Ok(Json(types::ApiResponse::success(
                "Reload initiated - controllers will restart with new server_id".to_string(),
            )))
        }
        Err(e) => {
            tracing::warn!(
                component = "admin_api",
                event = "reload_failed",
                error = %e,
                "Reload request failed"
            );
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// Manually trigger ACME certificate issuance or renewal for a specific resource.
///
/// POST /api/v1/services/acme/{namespace}/{name}/trigger
///
/// This resets the retry counter and re-evaluates the resource, same as modifying the CRD.
async fn trigger_acme(Path((namespace, name)): Path<(String, String)>) -> Json<ApiResponse<String>> {
    let key = format!("{}/{}", namespace, name);
    tracing::info!(
        component = "admin_api",
        event = "acme_trigger",
        key = %key,
        "ACME trigger requested via Admin API"
    );

    crate::core::controller::services::acme::notify_resource_changed(key.clone());

    Json(ApiResponse::success(format!("ACME check triggered for {}", key)))
}

// ============= Auth Status Helpers =============

/// Build the pre-computed auth status based on the active auth configuration.
///
/// When both OIDC and local auth are enabled, `auth_method` is "oidc+local".
fn build_auth_status(
    local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig>,
    auth_config: Option<&crate::core::common::auth::AdminAuthConfig>,
) -> crate::core::common::local_auth::AuthStatusResponse {
    use crate::core::common::local_auth::AuthStatusResponse;
    let oidc_enabled = auth_config.map_or(false, |c| c.enabled);
    let local_enabled = local_auth_config.map_or(false, |c| c.enabled);
    match (oidc_enabled, local_enabled) {
        (true, true) => AuthStatusResponse {
            auth_required: true,
            auth_method: "oidc+local".to_string(),
        },
        (true, false) => AuthStatusResponse {
            auth_required: true,
            auth_method: "oidc".to_string(),
        },
        (false, true) => AuthStatusResponse {
            auth_required: true,
            auth_method: "local".to_string(),
        },
        (false, false) => AuthStatusResponse {
            auth_required: false,
            auth_method: "none".to_string(),
        },
    }
}

// ============= Router Setup =============

/// Create the admin API router with unified K8s-style endpoints
pub fn create_admin_router(conf_mgr: Arc<ConfMgr>, schema_validator: Arc<SchemaValidator>) -> Router {
    let admin_state = Arc::new(AdminState {
        conf_mgr,
        schema_validator,
    });

    Router::new()
        // Health check (liveness)
        .route("/health", get(health_check))
        // Readiness check (ready to serve traffic - ConfigServer ready)
        .route("/ready", get(readiness_check))
        // Prometheus scrape endpoint (auth-skipped, K8s controller-manager convention)
        .route(
            "/metrics",
            get(crate::core::common::observe::metrics_api::metrics_handler),
        )
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
        .route("/api/v1/server-info", get(get_server_info))
        .route("/api/v1/reload", post(reload_all_resources))
        // RegionRoute PluginMetaData endpoints
        .route(
            "/api/v1/cluster-region-routes",
            get(region_route_handlers::list_cluster_region_plugin_metadata),
        )
        .route(
            "/api/v1/service-region-routes",
            get(region_route_handlers::list_service_region_plugin_metadata),
        )
        // New-URL aliases for RegionRoute failover
        .route(
            "/api/v1/cluster-region-routes/failover",
            post(region_route_handlers::cluster_plugin_metadata_failover),
        )
        .route(
            "/api/v1/service-region-routes/failover",
            post(region_route_handlers::service_plugin_metadata_failover),
        )
        // Services: ACME manual trigger
        .route("/api/v1/services/acme/{namespace}/{name}/trigger", post(trigger_acme))
        // ConfigServer endpoints (for edgion-cli --target server)
        .route("/configserver/{kind}/list", get(configserver_handlers::list_resources))
        .route("/configserver/{kind}", get(configserver_handlers::get_resource))
        .with_state(admin_state)
}

/// Serve the admin API on the specified address
pub async fn serve(
    conf_mgr: Arc<ConfMgr>,
    schema_validator: Arc<SchemaValidator>,
    addr: std::net::SocketAddr,
    auth_config: Option<&crate::core::common::auth::AdminAuthConfig>,
    local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig>,
) -> anyhow::Result<()> {
    let base_router = create_admin_router(conf_mgr, schema_validator);
    // Add local auth login/me endpoints (before auth middleware so login is accessible)
    let router_with_auth_routes = api::add_local_auth_routes(base_router, local_auth_config);
    // Apply unified auth middleware (tries OIDC first, falls back to local)
    // TODO(fix-01 task 5): replace this temporary derivation with `derive_require_auth`
    // (intent-based, includes auto-generate fallback). _auth_state is discarded here but
    // will be wired into handlers by tasks 3/4 (status endpoint) and task 6 (retry loop).
    let require_auth = auth_config.as_ref().is_some_and(|c| c.enabled)
        || local_auth_config.as_ref().is_some_and(|c| c.enabled);
    let (router_with_auth, _auth_state) =
        api::apply_unified_auth_layer(router_with_auth_routes, auth_config, local_auth_config, require_auth, "admin_api")?;
    let app = api::apply_cors_layer(router_with_auth);

    // Add GET /api/v1/auth/status — public, no auth required (middleware skips /api/v1/auth/*)
    let auth_status = build_auth_status(local_auth_config, auth_config);
    let status_router = Router::new().route(
        "/api/v1/auth/status",
        get(move || {
            let status = auth_status.clone();
            async move { axum::Json(ApiResponse::success(status)) }
        }),
    );
    let app = app.merge(status_router);

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
    conf_mgr: Arc<ConfMgr>,
    schema_validator: Arc<SchemaValidator>,
    addr: std::net::SocketAddr,
    auth_config: Option<&crate::core::common::auth::AdminAuthConfig>,
    local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig>,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let base_router = create_admin_router(conf_mgr, schema_validator);
    // Add local auth login/me endpoints (before auth middleware so login is accessible)
    let router_with_auth_routes = api::add_local_auth_routes(base_router, local_auth_config);
    // Apply unified auth middleware (tries OIDC first, falls back to local)
    // TODO(fix-01 task 5): replace this temporary derivation with `derive_require_auth`
    // (intent-based, includes auto-generate fallback). _auth_state is discarded here but
    // will be wired into handlers by tasks 3/4 (status endpoint) and task 6 (retry loop).
    let require_auth = auth_config.as_ref().is_some_and(|c| c.enabled)
        || local_auth_config.as_ref().is_some_and(|c| c.enabled);
    let (router_with_auth, _auth_state) =
        api::apply_unified_auth_layer(router_with_auth_routes, auth_config, local_auth_config, require_auth, "admin_api")?;
    let app = api::apply_cors_layer(router_with_auth);

    // Add GET /api/v1/auth/status — public, no auth required (middleware skips /api/v1/auth/*)
    let auth_status = build_auth_status(local_auth_config, auth_config);
    let status_router = Router::new().route(
        "/api/v1/auth/status",
        get(move || {
            let status = auth_status.clone();
            async move { axum::Json(ApiResponse::success(status)) }
        }),
    );
    let app = app.merge(status_router);

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
