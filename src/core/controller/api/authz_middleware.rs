//! Authorization middleware (a `route_layer`, so it never runs on the fallback)
//! and the federation identity-injection layer.
//!
//! # Layer order (local admin server)
//!
//! The final onion order achieved in `serve_with_shutdown` is:
//!
//! ```text
//! unified_auth  (outermost -- `.layer()` in compose_admin_routes, runs first)
//!   assign_superuser_if_authenticated  (route_layer, added after authz)
//!     authz_layer  (route_layer, added first -- innermost route_layer)
//!       handler
//! ```
//!
//! Because axum route_layers are applied in LIFO order (later `.route_layer()`
//! calls wrap around earlier ones), the code in `serve_with_shutdown` must
//! call `.route_layer(authz_layer)` first and `.route_layer(assign_superuser)`
//! second so that `assign_superuser` executes before `authz_layer`.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{FromRequestParts, MatchedPath, RawPathParams, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use http::{Request, StatusCode};

use super::authz_classifier::{bounded_kind_label, classify};
use crate::core::common::authz::{Authorizer, Role};

/// Paths that bypass authz entirely (mirror the authn skip set).
fn is_skipped(path: &str) -> bool {
    matches!(
        path,
        "/health" | "/ready" | "/metrics" | "/api/v1/auth/login" | "/api/v1/auth/logout" | "/api/v1/auth/status"
    )
}

/// Authorization route_layer middleware.
///
/// Reads the `Role` from request extensions (inserted by
/// `assign_superuser_if_authenticated` for local admin, or `inject_center_identity`
/// for the fed path). Classifies the request via `MatchedPath` + the `{kind}` path
/// param (extracted natively via `RawPathParams`), then calls `Authorizer::authorize`.
/// Returns 403 on deny; calls `next` on allow.
///
/// Skip-set paths (`/health`, `/ready`, `/metrics`, `/api/v1/auth/{login,logout,status}`)
/// bypass authz entirely (mirroring the authn skip set).
///
/// A missing `Role` extension or an unrecognized route both produce 403.
///
/// `MatchedPath` is read from request extensions rather than used as an extractor
/// argument because axum 0.8.x's `MatchedPath` does not implement
/// `OptionalFromRequestParts`, which is required for the `Option<MatchedPath>`
/// extractor-arg form to satisfy the `FromFn` `Service` bound.
pub async fn authz_layer(State(authorizer): State<Arc<Authorizer>>, req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if is_skipped(&path) {
        return next.run(req).await;
    }

    // Role is inserted by authn (Superuser via assign_superuser_if_authenticated)
    // or the fed inject layer (Center via inject_center_identity).
    // Missing role => deny (never allow anonymous requests through authz).
    let role = match req.extensions().get::<Role>() {
        Some(r) => r.clone(),
        None => return forbidden(),
    };

    // MatchedPath is inserted into request extensions by axum's router for every
    // matched route. Reading it from extensions (rather than using Option<MatchedPath>
    // as a middleware argument) avoids the OptionalFromRequestParts bound that
    // axum 0.8.x's MatchedPath does not satisfy.
    let matched_str = req.extensions().get::<MatchedPath>().map(|m| m.as_str().to_string());

    let method = req.method().clone();

    // Extract the {kind} path param via axum's native RawPathParams extractor.
    // RawPathParams yields already-percent-decoded (key, value) pairs, so authz
    // sees the exact same decoded kind value that the handler's Path extractor sees.
    // RawPathParams only implements FromRequestParts (not OptionalFromRequestParts),
    // so we invoke it directly on the request parts and treat failures as "no param".
    let (kind, req) = {
        let (mut parts, body) = req.into_parts();
        let kind = RawPathParams::from_request_parts(&mut parts, &())
            .await
            .ok()
            .and_then(|p| {
                p.iter()
                    .find_map(|(k, v)| if k == "kind" { Some(v.to_string()) } else { None })
            });
        (kind, Request::from_parts(parts, body))
    };

    let (verb, kind_str) = match classify(&method, matched_str.as_deref(), kind.as_deref()) {
        Some(vk) => vk,
        None => {
            // Unknown/absent route => deny. Use bounded "unknown" labels.
            crate::core::common::observe::fed_metrics::record_rbac_denied("unknown", "unknown");
            return forbidden();
        }
    };

    if authorizer.authorize(&role, verb, &kind_str).allowed() {
        next.run(req).await
    } else {
        // Bound the kind label: only emit known kinds, else "unknown".
        let kind_label = bounded_kind_label(&kind_str);
        crate::core::common::observe::fed_metrics::record_rbac_denied(verb.as_str(), kind_label);
        // Structured log: classified verb/kind + matched template only, no raw bytes.
        tracing::info!(
            component = "authz",
            verb = verb.as_str(),
            kind = kind_label,
            route = matched_str.as_deref().unwrap_or("?"),
            "Center request denied by RBAC"
        );
        forbidden()
    }
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "Forbidden").into_response()
}

/// Injects a fixed `Center` role on the federation path (independent of auth mode).
///
/// Wired onto the federation router in Task 6; not used on the local admin path.
pub async fn inject_center_identity(mut req: Request<Body>, next: Next) -> Response {
    req.extensions_mut().insert(Role::Center);
    next.run(req).await
}

/// Assign `Role::Superuser` to any request that reaches this layer.
///
/// This layer runs AFTER the unified_auth middleware (which is the outermost layer,
/// added by `compose_admin_routes`). By the time a request reaches this layer it
/// is either:
///   - carrying a validated identity (authn passed), or
///   - in no-auth mode (`require_auth = false`, authn lets everything through).
///
/// In both cases the request is legitimately "authenticated-or-no-auth", so assigning
/// `Role::Superuser` is correct and makes no-auth mode allow-all on the local server.
///
/// This layer is a `route_layer`, so it only fires for matched routes (never the
/// fallback). The authz middleware reads the `Role` extension inserted here.
pub(crate) async fn assign_superuser_if_authenticated(mut req: Request<Body>, next: Next) -> Response {
    req.extensions_mut().insert(crate::core::common::authz::Role::Superuser);
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get, Router};
    use http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::core::common::authz::{Authorizer, CenterPolicy, Role};
    use crate::core::common::config::{FedAllowRule, FedRbacConfig};

    /// Build a test router that:
    /// 1. Has an outer layer that inserts the given `Role` into request extensions
    ///    (simulating what authn + assign_superuser or inject_center_identity do).
    /// 2. Has `authz_layer` as a route_layer (so it only fires on matched routes,
    ///    not on the 404 fallback).
    fn test_router(role: Role, authorizer: Arc<Authorizer>) -> Router {
        Router::new()
            .route("/api/v1/cluster/{kind}", get(|| async { "ok" }))
            .route("/health", get(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn_with_state(authorizer.clone(), authz_layer))
            .layer(axum::middleware::from_fn(
                move |mut req: Request<Body>, next: axum::middleware::Next| {
                    let role = role.clone();
                    async move {
                        req.extensions_mut().insert(role);
                        next.run(req).await
                    }
                },
            ))
    }

    fn center_policy_list_httproute() -> Arc<Authorizer> {
        Arc::new(Authorizer::new(CenterPolicy::from_config(&FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["list".into()],
                kinds: vec!["HTTPRoute".into()],
            }],
        })))
    }

    /// A Superuser (authenticated local admin) must always be allowed through authz.
    #[tokio::test]
    async fn superuser_allowed() {
        let r = test_router(Role::Superuser, Arc::new(Authorizer::new(CenterPolicy::deny_all())));
        let resp = r
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/HTTPRoute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// A Center role with an empty (deny-all) policy must get 403.
    #[tokio::test]
    async fn center_denied_without_grant() {
        let r = test_router(Role::Center, Arc::new(Authorizer::new(CenterPolicy::deny_all())));
        let resp = r
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/HTTPRoute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// A Center role with a matching grant must be allowed.
    #[tokio::test]
    async fn center_allowed_with_grant() {
        let r = test_router(Role::Center, center_policy_list_httproute());
        let resp = r
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/HTTPRoute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Skip-set paths must always pass through regardless of role or policy.
    #[tokio::test]
    async fn health_is_never_denied() {
        let r = test_router(Role::Center, Arc::new(Authorizer::new(CenterPolicy::deny_all())));
        let resp = r
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// No-auth mode: when `assign_superuser_if_authenticated` runs before `authz_layer`
    /// (the real wiring in `serve_with_shutdown`), no externally injected `Role` is needed --
    /// the superuser layer inserts `Role::Superuser` itself, so a deny-all authorizer still
    /// returns 200. This proves no-auth mode is effectively allow-all on the local server.
    #[tokio::test]
    async fn no_auth_mode_superuser_inserted_by_assign_layer() {
        // Build a router that wires the two layers in the same order as serve_with_shutdown:
        //   authz_layer first (innermost route_layer), then assign_superuser on top (outermost).
        // No external role-insertion layer -- assign_superuser is the sole source of Role.
        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let router = Router::new()
            .route("/api/v1/cluster/{kind}", get(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn_with_state(authorizer, authz_layer))
            .route_layer(axum::middleware::from_fn(assign_superuser_if_authenticated));

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/HTTPRoute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Superuser overrides even a deny-all policy.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Authz fires before the readiness gate: an unauthorized request must receive 403
    /// (authz fired) and NOT 503 (the readiness gate). Wiring: authz is the outer
    /// route_layer wrapping an inner route_layer that always returns 503 (stub for
    /// `require_ready_middleware`). The request carries `Role::Center` with a deny-all
    /// authorizer, so authz rejects it before the inner 503 stub is ever reached.
    #[tokio::test]
    async fn authz_fires_before_readiness_gate() {
        use axum::middleware::from_fn;

        // Stub standing in for require_ready_middleware: always returns 503.
        async fn always_503(req: Request<Body>, next: axum::middleware::Next) -> Response {
            let _ = (req, next);
            (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response()
        }

        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));

        // Wiring mirrors the real router:
        //   inner route_layer: always-503 stub (innermost, added first)
        //   outer route_layer: authz (outermost route_layer, added second -- runs first)
        // Plus an outer layer that injects Role::Center (simulating inject_center_identity).
        let router = Router::new()
            .route("/api/v1/cluster/{kind}", get(|| async { "ok" }))
            .route_layer(from_fn(always_503))
            .route_layer(axum::middleware::from_fn_with_state(authorizer, authz_layer))
            .layer(from_fn(
                |mut req: Request<Body>, next: axum::middleware::Next| async move {
                    req.extensions_mut().insert(Role::Center);
                    next.run(req).await
                },
            ));

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/HTTPRoute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // authz denies before the 503 stub is reached.
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
