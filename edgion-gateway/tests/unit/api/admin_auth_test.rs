use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{HeaderMap, Request},
    middleware::from_fn_with_state,
    routing::get,
    Router,
};
use tower::ServiceExt;

const CONFIGURED_TOKEN: &str = "configured-admin-token-0123456789abcdef";

struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

fn middleware_router(state: AdminAuthState) -> Router {
    Router::new()
        .route("/admin", get(|| async { "handler reached" }))
        .layer(from_fn_with_state(state, admin_auth_middleware))
}

async fn send_request(state: AdminAuthState, auth_header: Option<&str>) -> TestResponse {
    let mut request = Request::builder().uri("/admin");
    if let Some(value) = auth_header {
        request = request.header(header::AUTHORIZATION, value);
    }
    let response = middleware_router(state)
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    TestResponse { status, headers, body }
}

fn digest_of(s: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(s.as_bytes()));
    out
}

#[test]
fn unconfigured_token_yields_unconfigured() {
    assert_eq!(evaluate(None, Some("Bearer anything")), AdminAuthOutcome::Unconfigured);
    // Even with no header, unconfigured wins (fail-close before auth check).
    assert_eq!(evaluate(None, None), AdminAuthOutcome::Unconfigured);
}

#[test]
fn correct_token_proceeds() {
    let d = digest_of("super-secret-token-value-1234567890");
    assert_eq!(
        evaluate(Some(&d), Some("Bearer super-secret-token-value-1234567890")),
        AdminAuthOutcome::Proceed
    );
}

#[test]
fn wrong_or_missing_token_is_unauthorized() {
    let d = digest_of("right-token-aaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(
        evaluate(Some(&d), Some("Bearer wrong-token")),
        AdminAuthOutcome::Unauthorized
    );
    assert_eq!(evaluate(Some(&d), None), AdminAuthOutcome::Unauthorized);
    assert_eq!(evaluate(Some(&d), Some("")), AdminAuthOutcome::Unauthorized);
    // empty bearer value must not authenticate
    assert_eq!(evaluate(Some(&d), Some("Bearer ")), AdminAuthOutcome::Unauthorized);
}

#[test]
fn bearer_scheme_is_case_insensitive_and_trims() {
    assert_eq!(parse_bearer_token(Some("Bearer tok")), Some("tok"));
    assert_eq!(parse_bearer_token(Some("bearer tok")), Some("tok"));
    assert_eq!(parse_bearer_token(Some("BEARER   tok  ")), Some("tok"));
    assert_eq!(parse_bearer_token(Some("Basic tok")), None);
    assert_eq!(parse_bearer_token(Some("Bearer")), None);
    assert_eq!(parse_bearer_token(Some("Bearer ")), None);
    // No whitespace separator between scheme and token must be rejected,
    // not parsed as the token "xyz".
    assert_eq!(parse_bearer_token(Some("Bearerxyz")), None);
    assert_eq!(parse_bearer_token(Some("Bearer\ttok")), Some("tok"));
    assert_eq!(parse_bearer_token(None), None);
}

#[test]
fn from_token_treats_empty_and_whitespace_as_unconfigured() {
    assert!(AdminAuthState::from_token(None).token_digest.is_none());
    assert!(AdminAuthState::from_token(Some("".into())).token_digest.is_none());
    assert!(AdminAuthState::from_token(Some("   ".into())).token_digest.is_none());
}

#[test]
fn from_token_stores_digest_of_trimmed_value() {
    let st = AdminAuthState::from_token(Some("  padded-token-aaaaaaaaaaaaaaaaaaaaa  ".into()));
    assert_eq!(st.token_digest, Some(digest_of("padded-token-aaaaaaaaaaaaaaaaaaaaa")));
}

#[tokio::test]
async fn rejected_credentials_share_bearer_challenge_without_echoing_tokens() {
    let state = AdminAuthState::from_token(Some(CONFIGURED_TOKEN.to_string()));
    let missing = send_request(state.clone(), None).await;
    assert_eq!(missing.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing.headers.get(header::WWW_AUTHENTICATE).unwrap(),
        r#"Bearer realm="edgion-gateway-admin""#
    );

    let malformed_token = "submitted-malformed-token-that-must-not-leak";
    let malformed = send_request(state.clone(), Some(&format!("BearerMalformed {malformed_token}"))).await;
    assert_eq!(malformed.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        malformed.headers.get(header::WWW_AUTHENTICATE).unwrap(),
        r#"Bearer realm="edgion-gateway-admin""#
    );
    assert!(!malformed.body.contains(malformed_token));

    let wrong_token = "submitted-wrong-token-that-must-not-leak";
    let wrong = send_request(state, Some(&format!("Bearer {wrong_token}"))).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        wrong.headers.get(header::WWW_AUTHENTICATE).unwrap(),
        r#"Bearer realm="edgion-gateway-admin""#
    );
    assert!(!wrong.body.contains(wrong_token));

    assert_eq!(missing.body, malformed.body);
    assert_eq!(malformed.body, wrong.body);
}

#[tokio::test]
async fn correct_token_reaches_handler() {
    let response = send_request(
        AdminAuthState::from_token(Some(CONFIGURED_TOKEN.to_string())),
        Some(&format!("Bearer {CONFIGURED_TOKEN}")),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, "handler reached");
}

#[tokio::test]
async fn unconfigured_returns_retryable_503_without_challenge_or_token_echo() {
    let submitted = "submitted-token-while-server-is-unconfigured";
    let response = send_request(AdminAuthState::from_token(None), Some(&format!("Bearer {submitted}"))).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers.get(header::RETRY_AFTER).unwrap(), "10");
    assert!(!response.headers.contains_key(header::WWW_AUTHENTICATE));
    assert!(!response.body.contains(submitted));
}
