mod types;

pub use types::{ApiResponse, ListResponse};

use axum::Router;
use tower_http::cors::{Any, CorsLayer};

/// Apply a permissive CORS layer to an Axum router.
///
/// Allows any origin, method, and header. Suitable for development and
/// scenarios where the Admin API is behind a gateway that enforces stricter policies.
pub fn apply_cors_layer(router: Router) -> Router {
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
    router.layer(cors)
}

/// Apply optional JWT auth middleware to a router.
///
/// When `auth_config` is `Some` and `enabled`, wraps the router with
/// JWT Bearer authentication. Otherwise returns the router unchanged.
pub fn apply_auth_layer(
    router: Router,
    auth_config: Option<&crate::core::common::auth::AdminAuthConfig>,
    component: &str,
) -> Router {
    let Some(auth_cfg) = auth_config else {
        tracing::info!(
            component = component,
            "Admin API authentication not configured (no [auth] section)"
        );
        return router;
    };

    if !auth_cfg.enabled {
        tracing::info!(component = component, "Admin API authentication disabled by config");
        return router;
    }

    if let Some(err) = auth_cfg.validate() {
        tracing::error!(
            component = component,
            error = err,
            "Invalid auth configuration, starting without auth"
        );
        return router;
    }

    let auth_state = crate::core::common::auth::AuthMiddlewareState::from_config(auth_cfg);
    tracing::info!(
        component = component,
        discovery = %auth_cfg.discovery,
        "Admin API authentication enabled"
    );
    router.layer(axum::middleware::from_fn_with_state(
        auth_state,
        crate::core::common::auth::jwt_auth_middleware,
    ))
}
