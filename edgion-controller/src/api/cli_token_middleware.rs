//! Controller token authentication middleware — the sole Admin API authn layer.
//!
//! On a recognized pre-shared bearer token (Bearer-only, case-insensitive) it
//! injects the token's `Restricted` role and `CliTokenIdentity` (for deny-audit).
//! Both the local admin token (allow-all wildcard policy) and the scoped Center
//! cli tokens live in the same `Authorizer` map, so this one middleware
//! authenticates both. An unrecognized / absent token injects nothing; the
//! downstream `authz_layer` then 403s the (role-less) request — except on the
//! static public paths, which it skips.
//!
//! # Bearer-only
//!
//! This middleware reads only the `Authorization` header (never a cookie). The
//! token-only model has no cookie/session concept.

use std::sync::Arc;

use axum::{body::Body, extract::State, middleware::Next, response::Response};
use http::Request;

use crate::authz::Authorizer;

/// Audit identity for a token-authenticated request.
///
/// Carries the human-readable token name (the local admin token's reserved
/// `"admin"`, or a `cli_tokens[].name` from config) for deny-log structured
/// fields. Inserted alongside `Role::Restricted` by `cli_token_pre_authn`.
#[derive(Clone)]
pub struct CliTokenIdentity {
    pub name: String,
}

/// Extract the Bearer token value from the `Authorization` header.
///
/// Matches `Bearer ` case-insensitively. Returns `None` when the header is
/// absent, is not a valid UTF-8 string, or does not start with `bearer `.
fn extract_bearer(req: &Request<Body>) -> Option<&str> {
    let val = req.headers().get(http::header::AUTHORIZATION)?.to_str().ok()?;
    if val.len() > 7 && val[..7].eq_ignore_ascii_case("bearer ") {
        Some(&val[7..])
    } else {
        None
    }
}

/// Controller token authentication middleware.
///
/// Layer order: 503 gate (outer) → `cli_token_pre_authn` → `authz_layer`
/// (route_layer) → handler.
///
/// A `Bearer` token present in the `Authorization` header is looked up via
/// `Authorizer::cli_lookup` (SHA-256 hash comparison). On a hit it injects
/// `Role::Restricted` and `CliTokenIdentity`. On a miss or no bearer it injects
/// nothing — `authz_layer` then denies any non-public route.
pub async fn cli_token_pre_authn(
    State(authorizer): State<Arc<Authorizer>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(tok) = extract_bearer(&req) {
        if let Some((role, name)) = authorizer.cli_lookup(tok) {
            req.extensions_mut().insert(role);
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
    use crate::config::{FedAllowRule, FedRbacConfig};

    #[test]
    fn bearer_extraction_borrows_header_and_preserves_exact_token() {
        for value in ["Bearer private-token", "bEaReR private-token", "Bearer  spaced-token "] {
            let req = Request::builder()
                .header(http::header::AUTHORIZATION, value)
                .body(Body::empty())
                .unwrap();
            let token = extract_bearer(&req).unwrap();
            let header = req.headers()[http::header::AUTHORIZATION].to_str().unwrap();
            assert_eq!(token, &value[7..]);
            assert!(std::ptr::eq(token.as_ptr(), header[7..].as_ptr()));
        }
        for value in ["", "Bearer", "Bearer ", "Basic private-token", " Bearer private-token"] {
            let req = Request::builder()
                .header(http::header::AUTHORIZATION, value)
                .body(Body::empty())
                .unwrap();
            assert!(extract_bearer(&req).is_none());
        }
        assert!(extract_bearer(&Request::new(Body::empty())).is_none());
    }

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

    /// A recognized Bearer token must inject `Role` and `CliTokenIdentity`.
    #[tokio::test]
    async fn cli_token_hit_injects_role_and_identity() {
        let az = az_with_token("tok1");
        let app = Router::new()
            .route(
                "/probe",
                get(|req: Request<Body>| async move {
                    let has_role = req.extensions().get::<crate::authz::Role>().is_some();
                    let has_id = req.extensions().get::<CliTokenIdentity>().is_some();
                    format!("{has_role}-{has_id}")
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
        assert_eq!(&body[..], b"true-true");
    }

    /// An unknown Bearer token injects no `Role` — the request falls through
    /// role-less and `authz_layer` will deny it.
    #[tokio::test]
    async fn unknown_bearer_injects_no_role() {
        let az = az_with_token("tok1");
        let app =
            Router::new()
                .route(
                    "/probe",
                    get(|req: Request<Body>| async move {
                        req.extensions().get::<crate::authz::Role>().is_some().to_string()
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
        assert_eq!(&body[..], b"false");
    }

    /// A valid token placed in a cookie (not the `Authorization` header) must
    /// NOT inject a `Role`. This middleware is Bearer-only.
    #[tokio::test]
    async fn cookie_not_honored_no_role_injected() {
        let az = az_with_token("tok1");
        let app =
            Router::new()
                .route(
                    "/probe",
                    get(|req: Request<Body>| async move {
                        req.extensions().get::<crate::authz::Role>().is_some().to_string()
                    }),
                )
                .layer(axum::middleware::from_fn_with_state(az.clone(), cli_token_pre_authn));

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
}
