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
    let app = Router::new()
        .route(
            "/probe",
            get(|req: Request<Body>| async move { req.extensions().get::<crate::authz::Role>().is_some().to_string() }),
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
    let app = Router::new()
        .route(
            "/probe",
            get(|req: Request<Body>| async move { req.extensions().get::<crate::authz::Role>().is_some().to_string() }),
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
