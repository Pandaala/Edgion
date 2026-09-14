//! Authorization middleware (a `route_layer`, so it never runs on the fallback)
//! and the federation identity-injection layer.
//!
//! # Layer order (local admin server)
//!
//! The final onion order achieved in `serve_with_shutdown` is:
//!
//! ```text
//! require_token_configured_gate  (outermost -- 503 when no token configured)
//!   cli_token_pre_authn  (injects Role::Restricted for a recognized Bearer token)
//!     authz_layer  (route_layer -- classifies + enforces the token's policy)
//!       handler
//! ```
//!
//! A request reaches a handler only if `cli_token_pre_authn` injected a `Role`
//! (from a valid token) and `authz_layer` allowed it. There is no allow-all
//! identity: an "admin" request carries a `Restricted` role whose policy is the
//! wildcard admin policy. The federation router uses the same `authz_layer` with
//! `inject_center_identity` supplying the Center `Restricted` role.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{FromRequestParts, MatchedPath, RawPathParams, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use http::{Request, StatusCode};

use super::authz_classifier::{bounded_kind_label, classify};
use crate::authz::{Authorizer, Role, Verb};

/// Paths that bypass authz entirely (the static public set — probes + public
/// auth endpoints). Delegates to the canonical source of truth so this can never
/// drift from the authentication layer's notion of "public". NEVER reference the
/// operator-configurable `skip_paths` here (fr-cauth-01).
///
/// The test-only diagnostic routes (`/api/v1/debug/store-stats`,
/// `/api/v1/admin/wipe-all`) are NOT skipped — they require standard auth like
/// any other admin route (callers pass a Bearer token). They are still only
/// mounted under test mode (`create_admin_router`).
fn is_skipped(path: &str) -> bool {
    crate::api::public_paths::is_static_public(path)
}

/// Authorization route_layer middleware.
///
/// Reads the `Role` from request extensions (inserted by `cli_token_pre_authn`
/// for a recognized token, or `inject_center_identity` for the fed path).
/// Classifies the request via `MatchedPath` + the `{kind}` path param (extracted
/// natively via `RawPathParams`), then calls `Authorizer::authorize`. Returns 403
/// on deny; calls `next` on allow.
///
/// Skip-set paths (`/health`, `/ready`, `/metrics`, `/api/v1/auth/status`) bypass
/// authz entirely (the static public set).
///
/// A missing `Role` extension (unauthenticated — no/unknown token) produces
/// 401; an unrecognized route or a policy denial produces 403.
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

    // Role is inserted by authn (cli_token_pre_authn for a recognized token)
    // or the fed inject layer (Center via inject_center_identity).
    // Missing role => unauthenticated (no/unknown token) => 401, never allow.
    // (Distinct from 403, which means authenticated-but-forbidden by policy.)
    let role = match req.extensions().get::<Role>() {
        Some(r) => r.clone(),
        None => return unauthorized(),
    };

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

    let Some((verb, kind_str)) = classify(&method, matched_str.as_deref(), kind.as_deref()) else {
        // Unknown/absent route => deny. Source is "unknown" because we cannot
        // determine whether this was a Center or cli-token request without a
        // matched route to classify.
        edgion_common::observe::fed_metrics::record_rbac_denied(
            "unknown",
            "unknown",
            edgion_common::observe::fed_metrics::labels::rbac_source::UNKNOWN,
        );
        return forbidden();
    };

    // Self-introspection is inherent once authentication has supplied a Role.
    // It is deliberately handled outside CenterPolicy so it cannot be granted,
    // revoked, or matched by an RBAC wildcard.
    if verb == Verb::AccessSelf {
        return next.run(req).await;
    }

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
                edgion_common::observe::fed_metrics::labels::rbac_source::CLI_TOKEN,
                id.name.as_str(),
            )
        } else {
            (
                edgion_common::observe::fed_metrics::labels::rbac_source::CENTER,
                "center",
            )
        };

        edgion_common::observe::fed_metrics::record_rbac_denied(verb.as_str(), kind_label, source);

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

/// 401 for an unauthenticated request (no `Role` — no/unknown token). Carries
/// `WWW-Authenticate: Bearer` per the HTTP spec for a bearer-token API.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(http::header::WWW_AUTHENTICATE, "Bearer")],
        "Unauthorized",
    )
        .into_response()
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get, Router};
    use http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::api::cli_token_middleware::cli_token_pre_authn;
    use crate::authz::{sha256_hex, Authorizer, CenterPolicy, CliTokenEntry, Role};
    use crate::config::{FedAllowRule, FedRbacConfig};

    /// Router with an outer layer injecting `role`, and `authz_layer` as a
    /// route_layer (so it only fires on matched routes, not the fallback).
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

    /// Router with NO role injected — only `authz_layer`. Simulates an
    /// unauthenticated request reaching authz.
    fn test_router_no_role(authorizer: Arc<Authorizer>) -> Router {
        Router::new()
            .route("/api/v1/cluster/{kind}", get(|| async { "ok" }))
            .route("/health", get(|| async { "ok" }))
            .route("/api/v1/auth/status", get(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn_with_state(authorizer, authz_layer))
    }

    fn center_policy_list_httproute() -> Arc<Authorizer> {
        Arc::new(Authorizer::new(CenterPolicy::from_config(&FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["list".into()],
                kinds: vec!["HTTPRoute".into()],
            }],
        })))
    }

    async fn status_of(router: Router, method: &str, uri: &str, bearer: Option<&str>) -> StatusCode {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = bearer {
            b = b.header("Authorization", format!("Bearer {t}"));
        }
        router.oneshot(b.body(Body::empty()).unwrap()).await.unwrap().status()
    }

    /// A Restricted role with an empty (deny-all) policy must get 403.
    #[tokio::test]
    async fn center_denied_without_grant() {
        let az = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let r = test_router(az.center_role(), az);
        assert_eq!(
            status_of(r, "GET", "/api/v1/cluster/HTTPRoute", None).await,
            StatusCode::FORBIDDEN
        );
    }

    /// A Restricted role with a matching grant must be allowed.
    #[tokio::test]
    async fn center_allowed_with_grant() {
        let az = center_policy_list_httproute();
        let r = test_router(az.center_role(), az);
        assert_eq!(
            status_of(r, "GET", "/api/v1/cluster/HTTPRoute", None).await,
            StatusCode::OK
        );
    }

    /// Skip-set paths must always pass through regardless of role or policy.
    #[tokio::test]
    async fn health_is_never_denied() {
        let az = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let r = test_router(az.center_role(), az);
        assert_eq!(status_of(r, "GET", "/health", None).await, StatusCode::OK);
    }

    /// An unauthenticated request (no `Role` injected) to a business route is
    /// 401 (no/unknown token) by authz_layer — there is no allow-all fallback.
    #[tokio::test]
    async fn no_role_is_unauthorized() {
        let az = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let r = test_router_no_role(az);
        assert_eq!(
            status_of(r, "GET", "/api/v1/cluster/HTTPRoute", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// Static public paths (probes, /api/v1/auth/status) pass authz with no role.
    #[tokio::test]
    async fn public_paths_pass_without_role() {
        let az = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        assert_eq!(
            status_of(test_router_no_role(az.clone()), "GET", "/health", None).await,
            StatusCode::OK
        );
        assert_eq!(
            status_of(test_router_no_role(az), "GET", "/api/v1/auth/status", None).await,
            StatusCode::OK
        );
    }

    /// End-to-end token authn → authz: a valid cli token is granted/denied per
    /// its policy; an unknown/absent token is role-less and denied (403).
    #[tokio::test]
    async fn e2e_token_authn_then_authz() {
        let az = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let p = CenterPolicy::from_config(&FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["list".into()],
                kinds: vec!["HTTPRoute".into()],
            }],
        });
        let mut m = std::collections::HashMap::new();
        m.insert(
            sha256_hex("tok1"),
            CliTokenEntry {
                name: "bot".into(),
                policy: Arc::new(p),
            },
        );
        az.apply(CenterPolicy::deny_all(), m);

        let app = Router::new()
            .route("/api/v1/cluster/{kind}", get(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn_with_state(az.clone(), authz_layer))
            .layer(axum::middleware::from_fn_with_state(az.clone(), cli_token_pre_authn));

        // (1) granted: token policy permits list HTTPRoute.
        assert_eq!(
            status_of(app.clone(), "GET", "/api/v1/cluster/HTTPRoute", Some("tok1")).await,
            StatusCode::OK
        );
        // (2) denied by policy: valid token, but list Secret is not granted.
        assert_eq!(
            status_of(app.clone(), "GET", "/api/v1/cluster/Secret", Some("tok1")).await,
            StatusCode::FORBIDDEN
        );
        // (3) no token: role-less → 401 (unauthenticated).
        assert_eq!(
            status_of(app, "GET", "/api/v1/cluster/HTTPRoute", None).await,
            StatusCode::UNAUTHORIZED
        );
    }
}
