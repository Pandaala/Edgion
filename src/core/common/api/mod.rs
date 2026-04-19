mod types;

pub use types::{ApiResponse, ListResponse};

use axum::{
    routing::{get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};

/// Build a `UnifiedAuthState` and wrap the router with the unified auth layer.
///
/// Returns both the wrapped router and the `Arc<UnifiedAuthState>` so the caller
/// can (a) pass it as sub-state to handlers that need runtime access to the
/// provider state (e.g. `build_auth_status`, login handlers) and (b) hand it to
/// a background task for lazy credential installation via `update_local`.
///
/// # Errors
///
/// Returns `Err` if any `enabled = true` provider's config fails validation —
/// callers should propagate the error so the process exits with non-zero status.
pub fn apply_unified_auth_layer(
    router: Router,
    auth_config: Option<&crate::core::common::auth::AdminAuthConfig>,
    local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig>,
    require_auth: bool,
    component: &str,
) -> anyhow::Result<(Router, std::sync::Arc<crate::core::common::unified_auth::UnifiedAuthState>)> {
    let state = crate::core::common::unified_auth::UnifiedAuthState::from_configs(
        auth_config,
        local_auth_config,
        require_auth,
        component,
    )?;
    let router = router.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::core::common::unified_auth::unified_auth_middleware,
    ));
    Ok((router, state))
}

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

/// Apply optional local auth middleware to a router.
///
/// When `local_auth_config` is `Some` and `enabled`, wraps the router with
/// local JWT authentication. Otherwise returns the router unchanged.
pub fn apply_local_auth_layer(
    router: Router,
    local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig>,
    component: &str,
) -> Router {
    let Some(local_auth_cfg) = local_auth_config else {
        tracing::info!(component = component, "Local auth not configured");
        return router;
    };

    if !local_auth_cfg.enabled {
        tracing::info!(component = component, "Local auth disabled by config");
        return router;
    }

    if let Some(err) = local_auth_cfg.validate() {
        tracing::error!(
            component = component,
            error = err,
            "Invalid local_auth configuration, starting without local auth"
        );
        return router;
    }

    let local_auth_state = crate::core::common::local_auth::LocalAuthState::from_config(local_auth_cfg);
    tracing::info!(
        component = component,
        username = %local_auth_cfg.username,
        "Admin API local authentication enabled"
    );
    router.layer(axum::middleware::from_fn_with_state(
        local_auth_state,
        crate::core::common::local_auth::local_auth_middleware,
    ))
}

/// Add local auth login/me routes to the router.
///
/// Only adds routes when `local_auth_config` is `Some` and `enabled`.
/// Routes added:
/// - `POST /api/v1/auth/login` — validate credentials, return JWT
/// - `GET  /api/v1/auth/me`   — return current user info (requires valid JWT)
///
/// The login/me routes are built as a separate sub-router with `LocalAuthState`
/// and then merged into the main router, which allows the two routers to have
/// different state types.
pub fn add_local_auth_routes(
    router: Router,
    local_auth_config: Option<&crate::core::common::local_auth::LocalAuthConfig>,
) -> Router {
    let Some(local_auth_cfg) = local_auth_config else {
        return router;
    };

    if !local_auth_cfg.enabled {
        return router;
    }

    use crate::core::common::local_auth::LocalAuthState;
    use std::sync::Arc;

    let local_auth_state = LocalAuthState::from_config(local_auth_cfg);

    // Build a separate sub-router with its own state, then merge it.
    let auth_router: Router<Arc<LocalAuthState>> = Router::new()
        .route(
            "/api/v1/auth/login",
            post(crate::core::common::local_auth::login_handler),
        )
        .route(
            "/api/v1/auth/logout",
            post(crate::core::common::local_auth::logout_handler),
        )
        .route("/api/v1/auth/me", get(crate::core::common::local_auth::me_handler));

    // `with_state` produces a `Router<()>` that can be merged into the main router.
    let auth_router_with_state: Router<()> = auth_router.with_state(local_auth_state);
    router.merge(auth_router_with_state)
}
