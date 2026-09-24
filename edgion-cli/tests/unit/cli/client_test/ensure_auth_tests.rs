use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

impl EdgionClient {
    /// Return the active bearer token string for test inspection.
    pub fn token_for_test(&self) -> Option<&str> {
        self.token.as_ref().map(|t| t.as_str())
    }
}

/// Build an `EdgionClient` pointed at `base_url` with no cached token.
fn make_client(base_url: String) -> EdgionClient {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .connect_timeout(std::time::Duration::from_secs(2))
        .build()
        .expect("reqwest client");
    EdgionClient {
        client,
        base_url,
        target: crate::cli::TargetType::Center,
        token: None,
    }
}

#[tokio::test]
async fn ensure_auth_bails_on_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let mut client = make_client(server.uri());
    let r = client.ensure_auth().await;
    assert!(r.is_err(), "404 must bail, got {:?}", r);
    let msg = format!("{:?}", r.unwrap_err());
    assert!(
        msg.contains("404") || msg.contains("too old"),
        "err message should mention 404 / too old: {msg}"
    );
}

#[tokio::test]
async fn ensure_auth_bails_when_ready_and_auth_required_but_no_token() {
    // Ready + auth_required and we reached ensure_auth (token-less path): must
    // bail with an actionable token message — the CLI never exchanges creds.
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "success": true,
        "data": {
            "auth_required": true,
            "mode": "token",
            "status": "ready",
        }
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let mut client = make_client(server.uri());
    let r = client.ensure_auth().await;
    assert!(r.is_err());
    let msg = format!("{:?}", r.unwrap_err()).to_lowercase();
    assert!(
        msg.contains("token") && (msg.contains("--token") || msg.contains("edgion_token")),
        "err should mention how to supply a token: {msg}"
    );
}

#[tokio::test]
async fn ensure_auth_continues_when_auth_not_required() {
    // Ready but auth_required = false: proceed without a token.
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "success": true,
        "data": {
            "auth_required": false,
            "mode": "token",
            "status": "ready",
        }
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let mut client = make_client(server.uri());
    let r = client.ensure_auth().await;
    assert!(r.is_ok(), "auth_required=false should continue, got {:?}", r);
}

#[tokio::test]
async fn ensure_auth_bails_on_5xx() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let mut client = make_client(server.uri());
    assert!(client.ensure_auth().await.is_err());
}

#[tokio::test]
async fn ensure_auth_bails_on_initializing() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "success": true,
        "data": {
            "auth_required": true,
            "mode": "token",
            "status": "initializing"
        }
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let mut client = make_client(server.uri());
    let r = client.ensure_auth().await;
    assert!(r.is_err());
    let msg = format!("{:?}", r.unwrap_err());
    assert!(
        msg.to_lowercase().contains("initializ"),
        "expected 'initializing' in err: {}",
        msg
    );
}

#[tokio::test]
async fn ensure_auth_continues_on_disabled() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "success": true,
        "data": {
            "auth_required": false,
            "mode": "disabled",
            "status": "disabled"
        }
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let mut client = make_client(server.uri());
    let r = client.ensure_auth().await;
    assert!(r.is_ok(), "disabled should continue, got {:?}", r);
}

#[tokio::test]
async fn ensure_auth_bails_on_malformed_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&server)
        .await;
    let mut client = make_client(server.uri());
    assert!(client.ensure_auth().await.is_err());
}

#[tokio::test]
async fn ensure_auth_bails_on_unreachable() {
    let mut client = make_client("http://127.0.0.1:1".to_string());
    assert!(client.ensure_auth().await.is_err());
}

/// `with_static_bearer` stores the supplied token and does not read the
/// file-cached token.
#[test]
fn with_static_bearer_sets_token() {
    let c = EdgionClient::with_static_bearer(crate::cli::TargetType::Center, None, None, "edg_testtoken")
        .expect("constructor");
    // The stored token must be the supplied bearer, not None
    assert_eq!(c.token_for_test(), Some("edg_testtoken"));
    let request = c
        .authenticate_request(c.client.get("http://localhost/"))
        .build()
        .expect("request must build without I/O");
    let authorization = &request.headers()[reqwest::header::AUTHORIZATION];
    assert_eq!(authorization, "Bearer edg_testtoken");
    assert!(authorization.is_sensitive());
    assert!(!format!("{request:?}").contains("edg_testtoken"));
}

/// `with_static_bearer` uses the supplied base_url and the token is set.
#[test]
fn with_static_bearer_uses_supplied_url() {
    let c = EdgionClient::with_static_bearer(
        crate::cli::TargetType::Center,
        Some("http://localhost:9999".to_string()),
        None,
        "edg_abc",
    )
    .expect("constructor");
    assert_eq!(c.base_url(), "http://localhost:9999");
    assert_eq!(c.token_for_test(), Some("edg_abc"));
}
