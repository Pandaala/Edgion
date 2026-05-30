//! Authorization middleware (a `route_layer`, so it never runs on the fallback)
//! and the federation identity-injection layer.
//!
//! # Layer order (local admin server)
//!
//! The final onion order achieved in `serve_with_shutdown` is:
//!
//! ```text
//! cli_token_pre_authn  (outermost -- outer `.layer()` on compose result)
//!   unified_auth  (skips when AuthBypass present -- `.layer()` in compose_admin_routes)
//!     assign_superuser_if_no_role  (route_layer, added after authz)
//!       authz_layer  (route_layer, added first -- innermost route_layer)
//!         handler
//! ```
//!
//! Because axum route_layers are applied in LIFO order (later `.route_layer()`
//! calls wrap around earlier ones), the code in `serve_with_shutdown` must
//! call `.route_layer(authz_layer)` first and `.route_layer(assign_superuser_if_no_role)`
//! second so that `assign_superuser_if_no_role` executes before `authz_layer`.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{FromRequestParts, MatchedPath, RawPathParams, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use http::{Request, StatusCode};

use super::authz_classifier::{bounded_kind_label, classify};
use crate::core::controller::authz::{Authorizer, Role};

/// Paths that bypass authz entirely (the static public set — probes + public
/// auth endpoints). Delegates to the canonical source of truth so this can never
/// drift from the authentication layer's notion of "public". NEVER reference the
/// operator-configurable `skip_paths` here (fr-cauth-01).
///
/// Under test/integration mode the test-only diagnostic routes
/// (`/api/v1/debug/store-stats`, `/api/v1/admin/wipe-all`) are additionally
/// skipped, mirroring the authn layer's `skip_paths` bypass for the same set
/// (see `unified_auth::UnifiedAuthState::from_configs`). Without this, the authn
/// layer waves these tokenless requests through (no claims), then
/// `assign_superuser_if_no_role` fail-closes them with 403 — the two layers
/// would contradict each other. This is safe in production: those routes are
/// only mounted under test mode (`create_admin_router`), so a production request
/// reaches the fallback (404) and never this route_layer.
fn is_skipped(path: &str) -> bool {
    is_skipped_with_mode(path, test_mode_enabled())
}

/// Pure decision used by [`is_skipped`], split out so the test-mode branch can be
/// exercised without mutating the process-global test-mode flag.
fn is_skipped_with_mode(path: &str, test_mode: bool) -> bool {
    use crate::core::common::auth::public_paths;
    public_paths::is_static_public(path) || (test_mode && public_paths::TEST_ONLY_PATHS.contains(&path))
}

/// True when the controller runs in test or integration mode — the only modes in
/// which the test-only diagnostic routes are mounted. Mirrors the flag check in
/// `unified_auth::UnifiedAuthState::from_configs`.
fn test_mode_enabled() -> bool {
    crate::core::common::config::is_test_mode() || crate::core::common::config::is_integration_testing_mode()
}

/// Authorization route_layer middleware.
///
/// Reads the `Role` from request extensions (inserted by
/// `assign_superuser_if_no_role` for local admin, or `inject_center_identity`
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

    // Role is inserted by authn (Superuser via assign_superuser_if_no_role)
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

    // Snapshot cli-token identity for the deny log BEFORE consuming `req` parts.
    let cli_identity = req
        .extensions()
        .get::<super::cli_token_middleware::CliTokenIdentity>()
        .cloned();

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
            // Unknown/absent route => deny. Source is "unknown" because we cannot
            // determine whether this was a Center or cli-token request without a
            // matched route to classify.
            crate::core::common::observe::fed_metrics::record_rbac_denied(
                "unknown",
                "unknown",
                crate::core::common::observe::fed_metrics::labels::rbac_source::UNKNOWN,
            );
            return forbidden();
        }
    };

    if authorizer.authorize(&role, verb, &kind_str).allowed() {
        next.run(req).await
    } else {
        // Bound the kind label: only emit known kinds, else "unknown".
        let kind_label = bounded_kind_label(&kind_str);

        // Determine the deny source from the cli-token identity snapshot:
        // cli-token-authenticated requests carry CliTokenIdentity; Center and
        // authenticated-admin requests do not. Token name goes in the LOG only —
        // NEVER as a metric label (unbounded cardinality).
        let (source, provider) = if let Some(ref id) = cli_identity {
            (
                crate::core::common::observe::fed_metrics::labels::rbac_source::CLI_TOKEN,
                id.name.as_str(),
            )
        } else {
            (
                crate::core::common::observe::fed_metrics::labels::rbac_source::CENTER,
                "center",
            )
        };

        crate::core::common::observe::fed_metrics::record_rbac_denied(verb.as_str(), kind_label, source);

        // Structured log: classified verb/kind + matched template + identity.
        // `token_name` is a log field only (never a metric label).
        if let Some(ref id) = cli_identity {
            tracing::info!(
                component = "authz",
                verb = verb.as_str(),
                kind = kind_label,
                route = matched_str.as_deref().unwrap_or("?"),
                provider = provider,
                token_name = id.name.as_str(),
                "cli token request denied by RBAC"
            );
        } else {
            tracing::info!(
                component = "authz",
                verb = verb.as_str(),
                kind = kind_label,
                route = matched_str.as_deref().unwrap_or("?"),
                provider = provider,
                "Center request denied by RBAC"
            );
        }
        forbidden()
    }
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "Forbidden").into_response()
}

/// Injects the Center role on the federation path (independent of auth mode).
///
/// Snapshots the current Center policy from the `Authorizer` and injects it as
/// `Role::Restricted`. Wired onto the federation router with
/// `from_fn_with_state(authorizer.clone(), inject_center_identity)`.
pub async fn inject_center_identity(
    State(authorizer): State<Arc<Authorizer>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    req.extensions_mut().insert(authorizer.center_role());
    next.run(req).await
}

/// Assign a `Role` to requests that don't already have one.
///
/// This layer runs AFTER `cli_token_pre_authn` (which may have already injected
/// `Role::Restricted`) and AFTER the `unified_auth` middleware (which has verified
/// the request is authenticated). It enforces fail-closed logic:
///
/// - If the path is in the public skip set (`is_skipped`: health/ready/metrics and
///   the auth endpoints) → pass through unchanged. These are intentionally
///   unauthenticated and carry no `Role`/claims; `authz_layer` skips them too.
/// - If a `Role` is already present in extensions (cli-pre injected `Restricted`) →
///   pass through unchanged.
/// - If no `Role` is present but `AuthBypass` IS present → the request was
///   "authenticated" by cli-pre but somehow carries no Role (invariant violation) →
///   **FAIL CLOSED: 403**. The bug path is made observable via `record_rbac_denied`.
///   We key the 403 on `AuthBypass` alone (NOT `AuthBypass && CliTokenIdentity`),
///   which prevents a future AuthBypass-without-CliTokenIdentity from silently
///   escalating to Superuser.
/// - If no `Role` and no `AuthBypass`, but `unified_auth` validated a token
///   (a `UnifiedAuthClaims` is present in extensions) → insert `Role::Superuser`,
///   granting full access on the local server.
/// - Otherwise (no Role, no AuthBypass, no `UnifiedAuthClaims`) the request reached
///   this route_layer WITHOUT being authenticated — the only way that happens is a
///   `unified_auth` skip_paths bypass on a business route → **FAIL CLOSED: 403**.
///   Granting Superuser here was the fr-cauth-01 escalation: an operator-configured
///   `skip_paths` entry on an admin route would otherwise turn an unauthenticated
///   request into a full admin one.
///
/// This layer is a `route_layer`, so it only fires for matched routes (never the
/// fallback). The authz middleware reads the `Role` extension inserted here.
pub(crate) async fn assign_superuser_if_no_role(mut req: Request<Body>, next: Next) -> Response {
    // Case 0: public skip-set paths (`/health`, `/ready`, `/metrics`,
    // `/api/v1/auth/{login,logout,status}`) bypass authz entirely — `authz_layer`
    // mirrors this same skip. These are intentionally unauthenticated infrastructure
    // endpoints, so they legitimately carry no `Role` and no `UnifiedAuthClaims`.
    // Without this short-circuit they would fall through to the fail-closed Case 4
    // below and wrongly 403 (this is NOT the fr-cauth-01 business-route bypass —
    // these paths are public by design). Pass through WITHOUT inserting a `Role`;
    // `authz_layer`'s own `is_skipped` then lets them reach the handler.
    if is_skipped(req.uri().path()) {
        return next.run(req).await;
    }

    // Case 1: cli-pre already assigned a Restricted role — keep it.
    if req.extensions().get::<crate::core::controller::authz::Role>().is_some() {
        return next.run(req).await;
    }

    // Case 2: AuthBypass present but no Role — invariant violation, fail closed.
    // Keyed on AuthBypass alone (not AuthBypass && CliTokenIdentity) so any future
    // AuthBypass source without a Role is also caught.
    if req
        .extensions()
        .get::<crate::core::common::unified_auth::AuthBypass>()
        .is_some()
    {
        // Emit metric so this bug path is observable.
        crate::core::common::observe::fed_metrics::record_rbac_denied(
            "unknown",
            "unknown",
            crate::core::common::observe::fed_metrics::labels::rbac_source::UNKNOWN,
        );
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }

    // Case 3: unified_auth actually validated a token (claims present) — assign Superuser.
    // The `UnifiedAuthClaims` check is the authentication proof: a request that skipped
    // unified_auth via skip_paths carries no claims and must NOT be treated as admin.
    if req
        .extensions()
        .get::<crate::core::common::unified_auth::UnifiedAuthClaims>()
        .is_some()
    {
        req.extensions_mut()
            .insert(crate::core::controller::authz::Role::Superuser);
        return next.run(req).await;
    }

    // Case 4: no Role, no AuthBypass, no claims — unauthenticated (skip_paths bypass on a
    // business route). Fail closed and make the bug path observable.
    crate::core::common::observe::fed_metrics::record_rbac_denied(
        "unknown",
        "unknown",
        crate::core::common::observe::fed_metrics::labels::rbac_source::UNKNOWN,
    );
    (StatusCode::FORBIDDEN, "Forbidden").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get, Router};
    use http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::core::common::config::{FedAllowRule, FedRbacConfig};
    use crate::core::controller::authz::{Authorizer, CenterPolicy, Role};

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

    /// A Restricted role with an empty (deny-all) policy must get 403.
    #[tokio::test]
    async fn center_denied_without_grant() {
        let az = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let r = test_router(az.center_role(), az);
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

    /// A Restricted role with a matching grant must be allowed.
    #[tokio::test]
    async fn center_allowed_with_grant() {
        let az = center_policy_list_httproute();
        let r = test_router(az.center_role(), az);
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
        let az = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let r = test_router(az.center_role(), az);
        let resp = r
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// When `assign_superuser_if_no_role` runs before `authz_layer`
    /// (the real wiring in `serve_with_shutdown`), an authenticated admin request
    /// (carrying `UnifiedAuthClaims` from unified_auth) with no explicit `Role`
    /// extension is assigned `Role::Superuser`, so a deny-all authorizer still
    /// returns 200.
    #[tokio::test]
    async fn assign_layer_inserts_superuser_when_no_role() {
        use crate::core::common::unified_auth::{AuthProvider, UnifiedAuthClaims};

        // Build a router that wires the two layers in the same order as serve_with_shutdown:
        //   authz_layer first (innermost route_layer), then assign_superuser on top (outermost).
        // An outer layer injects UnifiedAuthClaims, simulating a request that unified_auth
        // successfully authenticated (the only legitimate way to reach Case 3 with no Role).
        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let router = Router::new()
            .route("/api/v1/cluster/{kind}", get(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn_with_state(authorizer, authz_layer))
            .route_layer(axum::middleware::from_fn(assign_superuser_if_no_role))
            .layer(axum::middleware::from_fn(
                move |mut req: Request<Body>, next: axum::middleware::Next| async move {
                    req.extensions_mut().insert(UnifiedAuthClaims {
                        provider: AuthProvider::Local,
                        sub: None,
                        iss: None,
                        claims: serde_json::Value::Null,
                    });
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
        // Superuser overrides even a deny-all policy.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// fr-cauth-01 regression: a request that reaches the business route_layer with no
    /// `Role`, no `AuthBypass`, AND no `UnifiedAuthClaims` was never authenticated (it
    /// slipped past unified_auth via a misconfigured `skip_paths` entry on a business
    /// route). It must be denied (403), NOT silently granted Superuser.
    #[tokio::test]
    async fn no_claims_no_role_is_denied_not_superuser() {
        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let router = Router::new()
            .route("/api/v1/cluster/{kind}", get(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn_with_state(authorizer, authz_layer))
            .route_layer(axum::middleware::from_fn(assign_superuser_if_no_role));

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/HTTPRoute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // No authentication proof => fail closed, never Superuser.
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// Public skip-set paths (`/health`, `/ready`, `/metrics`, auth endpoints) must
    /// pass through `assign_superuser_if_no_role` even with no `Role`, no `AuthBypass`,
    /// and no `UnifiedAuthClaims` — they are intentionally unauthenticated infrastructure
    /// endpoints. The fr-cauth-01 fail-closed Case 4 must NOT swallow them with a 403.
    /// (Regression: a deny-all authorizer is wired to prove the skip happens before authz.)
    #[tokio::test]
    async fn skip_path_passes_through_without_claims() {
        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let router = Router::new()
            .route("/health", get(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn_with_state(authorizer, authz_layer))
            .route_layer(axum::middleware::from_fn(assign_superuser_if_no_role));

        let resp = router
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // Public skip path reaches the handler despite no auth proof and a deny-all policy.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// The test-only diagnostic routes are skipped by authz ONLY under test mode,
    /// mirroring the authn-layer `skip_paths` bypass. In production mode they are
    /// NOT skipped (defense in depth — though they are also never mounted there).
    /// The static-public set is skipped regardless of mode; business routes never.
    #[test]
    fn test_only_paths_skipped_only_in_test_mode() {
        use crate::core::common::auth::public_paths::TEST_ONLY_PATHS;

        // Static-public is mode-independent.
        assert!(is_skipped_with_mode("/health", false));
        assert!(is_skipped_with_mode("/health", true));

        // Test-only diagnostic routes: skipped in test mode, enforced otherwise.
        for p in TEST_ONLY_PATHS {
            assert!(is_skipped_with_mode(p, true), "{p} must be skipped in test mode");
            assert!(
                !is_skipped_with_mode(p, false),
                "{p} must be authz-enforced in production"
            );
        }

        // Business routes are never skipped, in either mode.
        assert!(!is_skipped_with_mode("/api/v1/cluster/httproute", true));
        assert!(!is_skipped_with_mode("/api/v1/cluster/httproute", false));
    }

    /// Verify the three fail-closed cases of `assign_superuser_if_no_role`:
    ///
    /// (a) `Role::Restricted(deny-all)` already in extensions → authz denies (403)
    /// (b) No Role, no AuthBypass, but `UnifiedAuthClaims` present (authenticated admin) → Superuser → allowed (200)
    /// (c) `AuthBypass` present but no Role (bug path) → 403, never Superuser
    #[tokio::test]
    async fn cli_role_kept_admin_gets_superuser_bypass_without_role_403() {
        use crate::core::common::unified_auth::{AuthBypass, AuthProvider, UnifiedAuthClaims};

        // Helper: build a router with:
        //   assign_superuser_if_no_role (outermost route_layer) → authz_layer (innermost)
        // Plus an outer `.layer` that injects whatever the test supplies via a closure.
        fn make_router(
            authorizer: Arc<Authorizer>,
            inject: impl Fn(&mut http::Extensions) + Clone + Send + Sync + 'static,
        ) -> Router {
            Router::new()
                .route("/api/v1/cluster/{kind}", get(|| async { "ok" }))
                .route_layer(axum::middleware::from_fn_with_state(authorizer.clone(), authz_layer))
                .route_layer(axum::middleware::from_fn(assign_superuser_if_no_role))
                .layer(axum::middleware::from_fn(
                    move |mut req: Request<Body>, next: axum::middleware::Next| {
                        let inject = inject.clone();
                        async move {
                            inject(req.extensions_mut());
                            next.run(req).await
                        }
                    },
                ))
        }

        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let uri = "/api/v1/cluster/HTTPRoute";

        // (a) Role::Restricted(deny-all) already present → authz denies → 403.
        {
            let center_role = authorizer.center_role(); // deny-all Restricted
            let r = make_router(authorizer.clone(), move |ext| {
                ext.insert(center_role.clone());
            });
            let resp = r
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::FORBIDDEN,
                "(a) Restricted deny-all must be 403"
            );
        }

        // (b) No Role, no AuthBypass, but unified_auth validated a token (claims present)
        //     → assign_superuser_if_no_role inserts Superuser → 200.
        {
            let r = make_router(authorizer.clone(), |ext| {
                ext.insert(UnifiedAuthClaims {
                    provider: AuthProvider::Local,
                    sub: None,
                    iss: None,
                    claims: serde_json::Value::Null,
                });
            });
            let resp = r
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "(b) no Role + no AuthBypass must be 200 (Superuser)"
            );
        }

        // (c) AuthBypass present but no Role → fail-closed → 403, never Superuser.
        {
            let r = make_router(authorizer.clone(), |ext| {
                ext.insert(AuthBypass);
                // Note: deliberately NOT inserting a Role.
            });
            let resp = r
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::FORBIDDEN,
                "(c) AuthBypass-without-Role must be fail-closed 403"
            );
        }
    }

    /// Authz fires before the readiness gate: an unauthorized request must receive 403
    /// (authz fired) and NOT 503 (the readiness gate). Wiring: authz is the outer
    /// route_layer wrapping an inner route_layer that always returns 503 (stub for
    /// `require_ready_middleware`). The request carries a deny-all `Restricted` role,
    /// so authz rejects it before the inner 503 stub is ever reached.
    #[tokio::test]
    async fn authz_fires_before_readiness_gate() {
        use axum::middleware::from_fn;

        // Stub standing in for require_ready_middleware: always returns 503.
        async fn always_503(req: Request<Body>, next: axum::middleware::Next) -> Response {
            let _ = (req, next);
            (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response()
        }

        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let center_role = authorizer.center_role();

        // Wiring mirrors the real router:
        //   inner route_layer: always-503 stub (innermost, added first)
        //   outer route_layer: authz (outermost route_layer, added second -- runs first)
        // Plus an outer layer that injects the center role (simulating inject_center_identity).
        let router = Router::new()
            .route("/api/v1/cluster/{kind}", get(|| async { "ok" }))
            .route_layer(from_fn(always_503))
            .route_layer(axum::middleware::from_fn_with_state(authorizer, authz_layer))
            .layer(from_fn(move |mut req: Request<Body>, next: axum::middleware::Next| {
                let role = center_role.clone();
                async move {
                    req.extensions_mut().insert(role);
                    next.run(req).await
                }
            }));

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

    /// End-to-end: wire cli-pre + compose + authz layers as `serve_with_shutdown` does,
    /// then assert:
    ///   (1) a cli token whose policy grants the op → not 403
    ///   (2) a cli token whose policy denies the op → 403
    ///   (3) no token, no admin credentials → 401 (unified_auth rejects, not authz)
    ///
    /// This test replicates the minimal subset of the real serving stack using
    /// `create_admin_router` + `compose_admin_routes` + cli-pre outer `.layer()`,
    /// matching the wiring in `serve_with_shutdown` exactly.
    #[tokio::test]
    async fn e2e_cli_token_granted_denied_and_no_token() {
        use crate::core::common::config::{FedAllowRule, FedRbacConfig};
        use crate::core::common::local_auth::LocalAuthConfig;
        use crate::core::controller::api::create_admin_router;
        use crate::core::controller::authz::{sha256_hex, CenterPolicy, CliTokenEntry};

        // ── Setup: build a real Authorizer with one cli token that can list GatewayClass.
        // GatewayClass is cluster-scoped, matching /api/v1/cluster/{kind}.
        let az = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let allowed_tok = "e2e-allowed-token";
        let denied_tok = "e2e-denied-token";
        {
            let allowed_policy = CenterPolicy::from_config(&FedRbacConfig {
                allow: vec![FedAllowRule {
                    verbs: vec!["list".into()],
                    kinds: vec!["GatewayClass".into()],
                }],
            });
            // denied_tok gets a deny-all policy (no allow rules).
            let denied_policy = CenterPolicy::deny_all();
            let mut m = std::collections::HashMap::new();
            m.insert(
                sha256_hex(allowed_tok),
                CliTokenEntry {
                    name: "allowed-bot".into(),
                    policy: Arc::new(allowed_policy),
                },
            );
            m.insert(
                sha256_hex(denied_tok),
                CliTokenEntry {
                    name: "denied-bot".into(),
                    policy: Arc::new(denied_policy),
                },
            );
            az.apply(CenterPolicy::deny_all(), m);
        }

        // ── Build the router stack identical to serve_with_shutdown.
        // Use a temp dir for the ConfMgr backing store.
        let tmp = tempfile::tempdir().expect("tempdir for e2e test");
        let base_router = {
            use crate::core::controller::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};
            let fs_config = FileSystemConfig::new(tmp.path().to_path_buf());
            let conf_mgr = Arc::new(
                ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
                    .await
                    .expect("ConfMgr::create"),
            );
            let schema_validator = Arc::new(SchemaValidator::empty());
            let (tx, _rx) = tokio::sync::watch::channel(crate::core::common::config::CenterClientConfig::default());
            create_admin_router(conf_mgr, schema_validator, None, az.clone(), tx)
        };

        // Wire authz route_layers (mirrors serve_with_shutdown exactly).
        let business = base_router
            .route_layer(axum::middleware::from_fn_with_state(az.clone(), authz_layer))
            .route_layer(axum::middleware::from_fn(assign_superuser_if_no_role));

        // Build a local_auth state so unified_auth is a real gatekeeper.
        let local_cfg = LocalAuthConfig {
            enabled: true,
            username: "admin".to_string(),
            password: "a_long_enough_password_123".to_string(),
            jwt_secret: "a_long_enough_jwt_secret_value_abcdef".to_string(),
            ..LocalAuthConfig::default()
        };
        let auth_state =
            crate::core::common::unified_auth::UnifiedAuthState::from_configs(None, Some(&local_cfg), true, "e2e_test")
                .expect("auth state");

        let composed = crate::core::common::api::compose_admin_routes(business, auth_state, true);

        // Wrap with cli-pre as the outermost layer (matching serve_with_shutdown).
        let app: Router = composed.layer(axum::middleware::from_fn_with_state(
            az.clone(),
            crate::core::controller::api::cli_token_middleware::cli_token_pre_authn,
        ));

        // We'll construct separate oneshotted requests by cloning the router.
        // GatewayClass is cluster-scoped, so GET /api/v1/cluster/GatewayClass is valid.
        let target = "/api/v1/cluster/GatewayClass";

        // (1) cli token with a matching grant → 200 OK.
        {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(target)
                        .header("Authorization", format!("Bearer {allowed_tok}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "allowed cli token must pass authz");
        }

        // (2) cli token with deny-all policy → 403.
        {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(target)
                        .header("Authorization", format!("Bearer {denied_tok}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::FORBIDDEN,
                "denied cli token must get 403 from authz"
            );
        }

        // (3) No token, no credentials → unified_auth rejects with 401 (not 403).
        {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(target).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "unauthenticated request must get 401 from unified_auth"
            );
        }
    }
}
