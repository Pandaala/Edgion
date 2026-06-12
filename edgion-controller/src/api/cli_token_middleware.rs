//! Controller cli-token pre-authn middleware. Runs OUTSIDE the shared
//! `unified_auth`. On a recognized cli token (Bearer-only, case-insensitive)
//! it injects the `Restricted` role, the common `AuthBypass` marker (so
//! `unified_auth` skips its own checks), and `CliTokenIdentity` (for
//! deny-audit). It only ADDS authentication; an unrecognized/absent token
//! falls through unchanged to `unified_auth`.
//!
//! # Auth-route skip
//!
//! Requests to `/api/v1/auth/*` are never processed by this middleware. A cli
//! token injecting `AuthBypass` on those paths would cause `unified_auth` to
//! skip its checks while the path's handler (e.g. `me_handler`) still expects
//! `Extension<UnifiedAuthClaims>` — which would be absent — producing a 500.
//! Instead, cli-token resolution is skipped entirely for auth routes so they
//! reach `unified_auth` unchanged.
//!
//! # Bearer-only
//!
//! This middleware reads only the `Authorization` header (never the
//! `edgion_token` cookie). Cookie-only callers are browser admins and fall
//! through to `unified_auth` as normal.

use std::sync::Arc;

use axum::{body::Body, extract::State, middleware::Next, response::Response};
use http::Request;

use crate::authz::Authorizer;
use edgion_common::unified_auth::AuthBypass;

/// Audit identity for a cli-token-authenticated request.
///
/// Carries the human-readable token name (from `cli_tokens[].name` in config)
/// for use in deny-log structured fields. Inserted alongside `Role::Restricted`
/// and `AuthBypass` by `cli_token_pre_authn` on a successful lookup.
#[derive(Clone)]
pub struct CliTokenIdentity {
    pub name: String,
}

/// Extract the Bearer token value from the `Authorization` header.
///
/// Matches `Bearer ` case-insensitively (parity with
/// `unified_auth::extract_bearer_token`). Returns `None` when the header is
/// absent, is not a valid UTF-8 string, or does not start with `bearer `.
///
/// Cookie tokens are intentionally NOT extracted here — cli-pre is Bearer-only.
fn extract_bearer(req: &Request<Body>) -> Option<String> {
    let val = req.headers().get(http::header::AUTHORIZATION)?.to_str().ok()?;
    if val.len() > 7 && val[..7].eq_ignore_ascii_case("bearer ") {
        Some(val[7..].to_string())
    } else {
        None
    }
}

/// Controller cli-token pre-authn middleware.
///
/// Layer order: `cli_token_pre_authn` (outer) → `unified_auth` (skips on
/// `AuthBypass`) → `assign_superuser_if_no_role` → `authz_layer` → handler.
///
/// # Behavior
///
/// 1. Paths under `/api/v1/auth/` pass through immediately (auth-route skip).
/// 2. A `Bearer` token present in the `Authorization` header is looked up via
///    `Authorizer::cli_lookup` (SHA-256 hash comparison).
/// 3. On hit: injects `Role::Restricted`, `AuthBypass`, and `CliTokenIdentity`.
/// 4. On miss or no bearer: falls through to `unified_auth` unchanged.
pub async fn cli_token_pre_authn(
    State(authorizer): State<Arc<Authorizer>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    // Skip auth routes entirely: letting cli-pre inject AuthBypass there would
    // cause unified_auth to skip, leaving UnifiedAuthClaims absent for handlers
    // that require it (e.g. /api/v1/auth/me -> 500 without this guard).
    if req.uri().path().starts_with("/api/v1/auth/") {
        return next.run(req).await;
    }

    if let Some(tok) = extract_bearer(&req) {
        if let Some((role, name)) = authorizer.cli_lookup(&tok) {
            req.extensions_mut().insert(role);
            req.extensions_mut().insert(AuthBypass);
            req.extensions_mut().insert(CliTokenIdentity { name });
        }
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::{body::Body, routing::get, Router};
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::authz::{sha256_hex, Authorizer, CenterPolicy, CliTokenEntry};
    use edgion_common::config::{FedAllowRule, FedRbacConfig};

    /// Build an `Authorizer` with a single cli token registered under `tok`.
    fn az_with_token(tok: &str) -> Arc<Authorizer> {
        let az = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
        let p = CenterPolicy::from_config(&FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["list".into()],
                kinds: vec!["HTTPRoute".into()],
            }],
        });
        let mut m = std::collections::HashMap::new();
        m.insert(
            sha256_hex(tok),
            CliTokenEntry {
                name: "bot".into(),
                policy: Arc::new(p),
            },
        );
        az.apply(CenterPolicy::deny_all(), m);
        az
    }

    /// A recognized Bearer token must inject `Role`, `AuthBypass`, and
    /// `CliTokenIdentity` into request extensions.
    #[tokio::test]
    async fn cli_token_hit_injects_role_bypass_identity() {
        let az = az_with_token("tok1");
        let app = Router::new()
            .route(
                "/probe",
                get(|req: Request<Body>| async move {
                    let has_role = req.extensions().get::<crate::authz::Role>().is_some();
                    let has_bypass = req
                        .extensions()
                        .get::<edgion_common::unified_auth::AuthBypass>()
                        .is_some();
                    let has_id = req.extensions().get::<CliTokenIdentity>().is_some();
                    format!("{has_role}-{has_bypass}-{has_id}")
                }),
            )
            .layer(axum::middleware::from_fn_with_state(az.clone(), cli_token_pre_authn));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header("Authorization", "Bearer tok1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        assert_eq!(&body[..], b"true-true-true");
    }

    /// An unknown Bearer token must NOT inject `AuthBypass` — the request falls
    /// through to `unified_auth` unchanged.
    #[tokio::test]
    async fn unknown_bearer_passes_through_untouched() {
        let az = az_with_token("tok1");
        let app = Router::new()
            .route(
                "/probe",
                get(|req: Request<Body>| async move {
                    req.extensions()
                        .get::<edgion_common::unified_auth::AuthBypass>()
                        .is_some()
                        .to_string()
                }),
            )
            .layer(axum::middleware::from_fn_with_state(az.clone(), cli_token_pre_authn));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header("Authorization", "Bearer not-a-cli-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        // No bypass injected — falls through to unified_auth.
        assert_eq!(&body[..], b"false");
    }

    /// A valid cli token placed in the `edgion_token` cookie (not the
    /// `Authorization` header) must NOT receive `AuthBypass`. The cli-pre
    /// middleware reads only the `Authorization` header; cookie callers are
    /// browser admins and handled by `unified_auth`.
    #[tokio::test]
    async fn cookie_not_honored_no_bypass_injected() {
        let az = az_with_token("tok1");
        let app = Router::new()
            .route(
                "/probe",
                get(|req: Request<Body>| async move {
                    req.extensions()
                        .get::<edgion_common::unified_auth::AuthBypass>()
                        .is_some()
                        .to_string()
                }),
            )
            .layer(axum::middleware::from_fn_with_state(az.clone(), cli_token_pre_authn));

        // Token is present — but in the cookie, not the Authorization header.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header("Cookie", "edgion_token=tok1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        assert_eq!(&body[..], b"false");
    }

    /// A request to `/api/v1/auth/me` carrying a valid cli token must NOT
    /// receive `AuthBypass` injected. The auth-route skip ensures that
    /// `unified_auth` still evaluates the request (and that `me_handler`'s
    /// `Extension<UnifiedAuthClaims>` extractor does not panic with a missing
    /// extension, which would produce a 500).
    #[tokio::test]
    async fn auth_me_path_is_skipped_no_bypass_injected() {
        let az = az_with_token("tok1");
        let app = Router::new()
            .route(
                "/api/v1/auth/me",
                get(|req: Request<Body>| async move {
                    req.extensions()
                        .get::<edgion_common::unified_auth::AuthBypass>()
                        .is_some()
                        .to_string()
                }),
            )
            .layer(axum::middleware::from_fn_with_state(az.clone(), cli_token_pre_authn));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/me")
                    .header("Authorization", "Bearer tok1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        // Auth-route skip: no bypass injected, unified_auth remains in control.
        assert_eq!(&body[..], b"false");
    }
}
