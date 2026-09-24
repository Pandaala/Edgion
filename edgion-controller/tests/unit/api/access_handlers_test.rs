use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use http::{Request, StatusCode};
use tower::ServiceExt;

use super::*;
use crate::api::authz_middleware::{authz_layer, inject_center_identity};
use crate::api::cli_token_middleware::cli_token_pre_authn;
use crate::authz::{sha256_hex, Authorizer, CenterPolicy, CliTokenEntry};
use crate::config::{FedAllowRule, FedRbacConfig};

fn access_route() -> Router {
    Router::new().route("/api/v1/access", get(get_access))
}

async fn request(app: Router, bearer: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().uri("/api/v1/access");
    if let Some(token) = bearer {
        request = request.header(http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn no_role_is_unauthorized_and_route_is_not_public() {
    let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
    let response = request(
        access_route().route_layer(from_fn_with_state(authorizer, authz_layer)),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[http::header::WWW_AUTHENTICATE], "Bearer");
}

#[tokio::test]
async fn federation_deny_all_role_can_introspect_itself() {
    let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
    let app = access_route()
        .route_layer(from_fn_with_state(authorizer.clone(), authz_layer))
        .layer(from_fn_with_state(authorizer, inject_center_identity));
    let response = request(app, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["resources"].as_array().unwrap().len(), 22);
    assert_eq!(body["data"]["operations"].as_array().unwrap().len(), 9);
}

#[tokio::test]
async fn local_token_uses_its_own_explicit_write_only_role() {
    let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
    let policy = CenterPolicy::from_config(&FedRbacConfig {
        allow: vec![FedAllowRule {
            verbs: vec!["create".into()],
            kinds: vec!["HTTPRoute".into()],
        }],
    });
    authorizer.apply(
        CenterPolicy::deny_all(),
        HashMap::from([(
            sha256_hex("write-token"),
            CliTokenEntry {
                name: "writer".into(),
                policy: Arc::new(policy),
            },
        )]),
    );
    let app = access_route()
        .route_layer(from_fn_with_state(authorizer.clone(), authz_layer))
        .layer(from_fn_with_state(authorizer, cli_token_pre_authn));
    let response = request(app, Some("write-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let route = body["data"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "HTTPRoute")
        .unwrap();
    assert_eq!(route["verbs"], serde_json::json!(["create"]));
}
