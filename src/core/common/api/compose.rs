//! Admin HTTP router composition.
//!
//! Provides `compose_admin_routes` -- the single shared composition entry
//! point for Controller and Center. The function assembles business routes,
//! shared auth endpoints (login/me/logout/status), metrics, health/ready,
//! and the unified_auth middleware in a fixed order. The `Router` returned
//! to the caller is treated as final; any subsequent routes would bypass
//! the middleware -- see the function-level docs for the final-state
//! contract.
//!
//! The Admin API does not mount a CORS layer: local development is handled
//! by edgion-dashboard's Vite `server.proxy` (see
//! `edgion-dashboard/vite.config.ts`), and in production the dashboard is
//! embedded and deployed same-origin with the Controller. If a separate
//! origin deployment becomes necessary in the future, introduce a precise
//! origin allowlist at that time.

use std::sync::Arc;

use axum::Router;

use super::{
    add_auth_status_route, add_local_auth_routes, cache_control_no_store_middleware, wrap_with_unified_auth_layer,
};
use crate::core::common::unified_auth::UnifiedAuthState;

/// Compose the admin HTTP Router in a fixed order.
///
/// # Composition order (hard-coded inside this function; not caller-controllable)
/// 1. `business` (each component's own business routes)
/// 2. `/api/v1/auth/{login, logout, me}` (shared local_auth handlers,
///    mounted only when `local_auth_intent = true`)
/// 3. `/api/v1/auth/status` (mounted unconditionally)
/// 4. `unified_auth` middleware (wraps all routes; skip_paths are provided by `auth_state`)
/// 5. `Cache-Control: no-store` middleware (outermost — see `cache_control_no_store_middleware`)
///
/// # Final-state contract
/// **The returned `Router` is final.** Do not call `.route()` / `.layer()`
/// on it -- any addition would bypass the auth middleware. New admin routes
/// should be added to the `business` argument before calling this function.
///
/// # Arguments
/// - `business`: per-component private business routes. **Must not** include
///   the auth routes mounted by this function (duplicate merges trigger an
///   axum panic). Whether business routes mount their own `/health` /
///   `/ready` / `/metrics` is up to the caller; this function does not mount
///   them.
/// - `auth_state`: shared authentication state from `UnifiedAuthState::from_configs`.
/// - `local_auth_intent`: whether local auth is intended to be enabled. When
///   `true`, mounts `/auth/{login,logout,me}`; when `false`, omits them
///   (OIDC-only or explicitly disabled). Note: this argument is not derived
///   from `auth_state`. The reason is that the Controller's K8s Secret
///   auto-generation path allows "intended-on but provider not yet ready"
///   (`auth_state.local` is `None` but intent is `true`); in that state the
///   routes should still be mounted (handlers return 503 until the Secret is
///   ready). `auth_state.local.load().is_some()` cannot distinguish this
///   case from "explicitly disabled", so the independent intent parameter is
///   retained.
pub fn compose_admin_routes(business: Router, auth_state: Arc<UnifiedAuthState>, local_auth_intent: bool) -> Router {
    let with_auth_routes = add_local_auth_routes(business, auth_state.clone(), local_auth_intent);
    let with_status = add_auth_status_route(with_auth_routes, auth_state.clone());
    let with_auth_layer = wrap_with_unified_auth_layer(with_status, auth_state);
    with_auth_layer.layer(axum::middleware::from_fn(cache_control_no_store_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    use crate::core::common::local_auth::LocalAuthConfig;

    fn valid_local() -> LocalAuthConfig {
        LocalAuthConfig {
            username: "admin".to_string(),
            password: "a_long_enough_password_123".to_string(),
            jwt_secret: "a_long_enough_jwt_secret_value_abcdef".to_string(),
            ..LocalAuthConfig::default()
        }
    }

    fn new_state_with_local() -> Arc<UnifiedAuthState> {
        let cfg = valid_local();
        UnifiedAuthState::from_configs(None, Some(&cfg), true, "test").unwrap()
    }

    fn dummy_business() -> Router {
        Router::new().route("/foo", get(|| async { "foo" }))
    }

    /// T1 -- after compose, /auth/{login,logout,me,status} are all routable (not 404).
    #[tokio::test]
    async fn compose_mounts_all_shared_routes() {
        let state = new_state_with_local();
        let app = compose_admin_routes(dummy_business(), state, true);

        for path in &[
            "/api/v1/auth/login",
            "/api/v1/auth/logout",
            "/api/v1/auth/me",
            "/api/v1/auth/status",
        ] {
            let req = Request::builder().uri(*path).body(Body::empty()).unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_ne!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "route {} should be mounted, got {:?}",
                path,
                resp.status()
            );
        }
    }

    /// T2 -- /auth/me must be 401 without a Bearer token (not 200, 500, or 503).
    #[tokio::test]
    async fn me_requires_auth() {
        let state = new_state_with_local();
        let app = compose_admin_routes(dummy_business(), state, true);
        let req = Request::builder().uri("/api/v1/auth/me").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// T3 -- /auth/status returns 200 without a token (the status endpoint must not be blocked by middleware).
    #[tokio::test]
    async fn status_does_not_require_auth() {
        let state = new_state_with_local();
        let app = compose_admin_routes(dummy_business(), state, true);
        let req = Request::builder()
            .uri("/api/v1/auth/status")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// T4 -- the business route /foo is 401 without a token (require_auth=true with a local provider).
    #[tokio::test]
    async fn business_route_requires_auth_when_enabled() {
        let state = new_state_with_local();
        let app = compose_admin_routes(dummy_business(), state, true);
        let req = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// T5 -- when require_auth=false, business routes pass through (no OIDC, no local).
    #[tokio::test]
    async fn business_route_passes_when_require_auth_false() {
        let state = UnifiedAuthState::from_configs(None, None, false, "test").unwrap();
        let app = compose_admin_routes(dummy_business(), state, false);
        let req = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// T6 -- every admin response carries `Cache-Control: no-store`.
    ///
    /// Verifies that the blanket middleware fires on:
    ///   - an authenticated 200 response (business route with auth disabled),
    ///   - a 401 rejection (auth required but no token supplied),
    ///   - a public endpoint (/api/v1/auth/status) that returns 200 without auth.
    #[tokio::test]
    async fn all_responses_carry_cache_control_no_store() {
        // Case A: auth disabled, business route → 200
        let state_no_auth = UnifiedAuthState::from_configs(None, None, false, "test").unwrap();
        let app_no_auth = compose_admin_routes(dummy_business(), state_no_auth, false);
        let req = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let resp = app_no_auth.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CACHE_CONTROL)
                .map(|v| v.as_bytes()),
            Some(b"no-store".as_slice()),
            "200 response must carry Cache-Control: no-store"
        );

        // Case B: auth required, no token → 401
        let state_auth = new_state_with_local();
        let app_auth = compose_admin_routes(dummy_business(), state_auth, true);
        let req = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let resp = app_auth.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CACHE_CONTROL)
                .map(|v| v.as_bytes()),
            Some(b"no-store".as_slice()),
            "401 response must carry Cache-Control: no-store"
        );

        // Case C: /api/v1/auth/status → 200 (no auth required)
        let state_auth2 = new_state_with_local();
        let app_auth2 = compose_admin_routes(dummy_business(), state_auth2, true);
        let req = Request::builder()
            .uri("/api/v1/auth/status")
            .body(Body::empty())
            .unwrap();
        let resp = app_auth2.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CACHE_CONTROL)
                .map(|v| v.as_bytes()),
            Some(b"no-store".as_slice()),
            "/auth/status response must carry Cache-Control: no-store"
        );
    }
}
