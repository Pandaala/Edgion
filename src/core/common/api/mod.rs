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
) -> anyhow::Result<(
    Router,
    std::sync::Arc<crate::core::common::unified_auth::UnifiedAuthState>,
)> {
    let state = crate::core::common::unified_auth::UnifiedAuthState::from_configs(
        auth_config,
        local_auth_config,
        require_auth,
        component,
    )?;
    let router = wrap_with_unified_auth_layer(router, state.clone());
    Ok((router, state))
}

/// Wrap an already-built router with the unified auth middleware using a
/// pre-constructed `UnifiedAuthState`.
///
/// **Call this AFTER all `.merge()` of auth-gated sub-routers** — axum's
/// `.layer()` only wraps routes already present in the router at call time;
/// sub-routes merged in later would otherwise bypass the middleware and
/// silently serve unauthenticated.
///
/// The typical pattern is:
///
/// ```ignore
/// let state = UnifiedAuthState::from_configs(...)?;
/// let app = base_router;
/// let app = add_local_auth_routes(app, state.clone(), intent); // /auth/login, /auth/logout, /auth/me
/// let app = app.merge(status_router);                           // /auth/status (reads state)
/// let app = wrap_with_unified_auth_layer(app, state.clone());   // now layer protects everything
/// ```
pub fn wrap_with_unified_auth_layer(
    router: Router,
    state: std::sync::Arc<crate::core::common::unified_auth::UnifiedAuthState>,
) -> Router {
    router.layer(axum::middleware::from_fn_with_state(
        state,
        crate::core::common::unified_auth::unified_auth_middleware,
    ))
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

/// Add local auth login/me/logout routes to the router using the shared
/// `UnifiedAuthState`. Handlers read the local provider state through the
/// state's `ArcSwapOption`, so routes stay valid even before the Secret
/// is fetched — they return 503 until the state is populated.
///
/// Routes are only added when the operator intends local auth to be enabled
/// (i.e. `[local_auth].enabled = true` or auto-generate fallback). When the
/// operator explicitly disabled local auth, the routes are omitted.
pub fn add_local_auth_routes(
    router: Router,
    unified_state: std::sync::Arc<crate::core::common::unified_auth::UnifiedAuthState>,
    local_auth_intent: bool,
) -> Router {
    if !local_auth_intent {
        return router;
    }

    use crate::core::common::local_auth::{login_handler, logout_handler, me_handler};

    let auth_router: Router<std::sync::Arc<crate::core::common::unified_auth::UnifiedAuthState>> = Router::new()
        .route("/api/v1/auth/login", post(login_handler))
        .route("/api/v1/auth/logout", post(logout_handler))
        .route("/api/v1/auth/me", get(me_handler));

    let auth_router_with_state: Router<()> = auth_router.with_state(unified_state);
    router.merge(auth_router_with_state)
}
