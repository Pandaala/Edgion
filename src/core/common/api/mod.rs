pub mod compose;
mod types;

pub use compose::compose_admin_routes;
pub use types::{ApiResponse, ListResponse};

use axum::{
    http::{header, HeaderValue, Response},
    middleware::Next,
    routing::{get, post},
    Router,
};

/// Inject `Cache-Control: no-store` on every admin API response.
///
/// Private caches (browsers, local proxies) are NOT bound by RFC 9111 §3.5's
/// prohibition on storing responses with an `Authorization` header, so the
/// server must make its intent explicit. Applied as the outermost layer in
/// `compose_admin_routes` so it fires unconditionally on every response path.
pub(crate) async fn cache_control_no_store_middleware(
    request: axum::extract::Request,
    next: Next,
) -> Response<axum::body::Body> {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Merge `GET /api/v1/auth/status` into the router, computed per-request
/// from the shared `UnifiedAuthState`. The handler re-reads `ArcSwapOption`
/// each call so post-init transitions (Initializing → Ready) become visible
/// to clients without a server restart.
///
/// Call this BEFORE `wrap_with_unified_auth_layer` so the status path exists
/// when the middleware layer wraps the whole router (the middleware then
/// exact-match bypasses the path for the client contract).
pub(crate) fn add_auth_status_route(
    router: Router,
    unified_state: std::sync::Arc<crate::core::common::unified_auth::UnifiedAuthState>,
) -> Router {
    let status_router: Router = Router::new().route(
        "/api/v1/auth/status",
        axum::routing::get(move || {
            let state = unified_state.clone();
            async move { axum::Json(ApiResponse::ok_body(state.build_status())) }
        }),
    );
    router.merge(status_router)
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
pub(crate) fn wrap_with_unified_auth_layer(
    router: Router,
    state: std::sync::Arc<crate::core::common::unified_auth::UnifiedAuthState>,
) -> Router {
    router.layer(axum::middleware::from_fn_with_state(
        state,
        crate::core::common::unified_auth::unified_auth_middleware,
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
pub(crate) fn add_local_auth_routes(
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
