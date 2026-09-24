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
