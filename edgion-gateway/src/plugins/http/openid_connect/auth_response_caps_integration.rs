//! Explicitly run network integration tests for OIDC authentication response caps.
//!
//! Standalone topology exception: these tests need only an ephemeral loopback IdP
//! and the real plugin endpoint methods, not another Gateway/Controller process.
//! They are ignored in ordinary unit runs because they perform network IO.
//! Run with: cargo test -p edgion-gateway --lib auth_response_caps_integration -- --ignored
//! Source: tasks/todo/oidc-auth-response-byte-caps.md (D0036).

use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const EXPECTED_CAP: usize = 1024 * 1024;

#[derive(Clone, Copy)]
enum Endpoint {
    Exchange,
    Refresh,
    Introspection,
    AutoIntrospection,
}

async fn call_endpoint(endpoint: Endpoint, size: usize, chunked: bool) -> VerifyResult<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let mut body = match endpoint {
        Endpoint::Exchange | Endpoint::Refresh => br#"{"access_token":"fixture-token","expires_in":3600}"#.to_vec(),
        Endpoint::Introspection | Endpoint::AutoIntrospection => {
            format!(r#"{{"active":true,"iss":"{origin}","sub":"fixture-user"}}"#).into_bytes()
        }
    };
    // JSON whitespace makes the size exact without depending on token validation
    // or the size of a particular credential field. The whole wire body is capped.
    assert!(body.len() <= size);
    body.resize(size, b' ');
    let mut server = tokio::spawn(async move {
        if matches!(endpoint, Endpoint::AutoIntrospection) {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
                assert!(request.len() < 16384);
            }
            assert!(request.starts_with(b"GET /jwks HTTP/1.1"));
            let jwks = br#"{"keys":[{"kty":"oct","kid":"fixture","k":"Zml4dHVyZS1rZXk","alg":"HS256"}]}"#;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", jwks.len()).as_bytes()).await.unwrap();
            socket.write_all(jwks).await.unwrap();
            socket.shutdown().await.unwrap();
        }
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0, "client closed before sending request headers");
            request.extend_from_slice(&buf[..n]);
            if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                assert!(headers.starts_with("POST /endpoint HTTP/1.1"));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                while request.len() < end + 4 + length {
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0, "client closed before sending request body");
                    request.extend_from_slice(&buf[..n]);
                }
                break;
            }
            assert!(request.len() < 16384, "fixture request headers too large");
        }
        let framing = if chunked {
            "Transfer-Encoding: chunked\r\n".to_string()
        } else {
            format!("Content-Length: {}\r\n", body.len())
        };
        // Early disconnect is expected once the client enforces the cap.
        let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n{framing}\r\n");
        if socket.write_all(header.as_bytes()).await.is_err() {
            return;
        }
        for part in body.chunks(8192) {
            if chunked
                && socket
                    .write_all(format!("{:x}\r\n", part.len()).as_bytes())
                    .await
                    .is_err()
            {
                return;
            }
            if socket.write_all(part).await.is_err() {
                return;
            }
            if chunked && socket.write_all(b"\r\n").await.is_err() {
                return;
            }
        }
        if chunked {
            let _ = socket.write_all(b"0\r\n\r\n").await;
        }
        let _ = socket.shutdown().await;
    });
    let config = OpenidConnectConfig {
        discovery: format!("{origin}/discovery"),
        client_id: "fixture-client".to_string(),
        resolved_oidc_client_secret: Some("fixture-secret".to_string()),
        bearer_only: true,
        verification_mode: if matches!(endpoint, Endpoint::AutoIntrospection) {
            VerificationMode::Auto
        } else {
            VerificationMode::IntrospectionOnly
        },
        use_jwks: true,
        allowed_signing_algs: Some(vec!["HS256".to_string()]),
        timeout: 3,
        ..Default::default()
    };
    let plugin = OpenidConnect::new(&config, "default".to_string());
    let discovery = DiscoveryDocument {
        issuer: origin.clone(),
        jwks_uri: format!("{origin}/jwks"),
        authorization_endpoint: None,
        token_endpoint: Some(format!("{origin}/endpoint")),
        introspection_endpoint: Some(format!("{origin}/endpoint")),
        userinfo_endpoint: None,
        end_session_endpoint: None,
        revocation_endpoint: None,
    };
    plugin.jwks_provider.seed_discovery(discovery.clone()).await;
    let result = match endpoint {
        Endpoint::Exchange => plugin
            .exchange_code_for_token(&discovery, "fixture-code", "https://client.example/callback", None)
            .await
            .map(|_| ()),
        Endpoint::Refresh => plugin
            .refresh_access_token(&discovery, "fixture-refresh")
            .await
            .map(|_| ()),
        Endpoint::Introspection => plugin
            .verify_token_via_introspection(&mut MockPluginSession::new(), "fixture-token")
            .await
            .map(|_| ()),
        Endpoint::AutoIntrospection => {
            // The signature is valid, but claims must be an object. Deserializing
            // a JSON string triggers InvalidFormat after signature verification.
            let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
            header.kid = Some("fixture".to_string());
            let token = jsonwebtoken::encode(
                &header,
                &serde_json::json!("not a claims object"),
                &jsonwebtoken::EncodingKey::from_secret(b"fixture-key"),
            )
            .unwrap();
            assert!(OpenidConnect::looks_like_jwt(&token));
            plugin
                .verify_token(&mut MockPluginSession::new(), &token)
                .await
                .map(|_| ())
        }
    };
    // A deadline also bounds a fixture waiting for a request that never arrived.
    match tokio::time::timeout(Duration::from_secs(5), &mut server).await {
        Ok(joined) => joined.expect("IdP fixture failed"),
        Err(_) => {
            server.abort();
            let _ = server.await;
            panic!("IdP fixture did not finish; endpoint result: {result:?}");
        }
    }
    result
}

macro_rules! cap_test {
    ($name:ident, $endpoint:expr, $chunked:expr) => {
        #[tokio::test]
        #[ignore = "network integration test: explicit local IdP fixture"]
        async fn $name() {
            call_endpoint($endpoint, EXPECTED_CAP, $chunked)
                .await
                .expect("exact-limit valid response must succeed");
            let result = call_endpoint($endpoint, EXPECTED_CAP + 1, $chunked).await;
            assert!(
                result.is_err(),
                "OIDC accepted a valid JSON response larger than the 1 MiB cap"
            );
            let error = result.unwrap_err();
            assert_eq!(error.http_status(), 502);
            assert_eq!(error.user_message(), "Bad Gateway");
        }
    };
}

cap_test!(exchange_declared_length_cap, Endpoint::Exchange, false);
cap_test!(exchange_chunked_cap, Endpoint::Exchange, true);
cap_test!(refresh_declared_length_cap, Endpoint::Refresh, false);
cap_test!(refresh_chunked_cap, Endpoint::Refresh, true);
cap_test!(introspection_declared_length_cap, Endpoint::Introspection, false);
cap_test!(introspection_chunked_cap, Endpoint::Introspection, true);

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn auto_introspection_keeps_401_and_response_diagnostic() {
    call_endpoint(Endpoint::AutoIntrospection, EXPECTED_CAP, true)
        .await
        .unwrap();
    let error = call_endpoint(Endpoint::AutoIntrospection, EXPECTED_CAP + 1, true)
        .await
        .unwrap_err();
    assert_eq!(error.http_status(), 401);
    assert_eq!(error.user_message(), "Invalid token format");
    assert_eq!(error.log_tag(), "auth response too large");
    assert_eq!(
        error.response_log_tag_or("token verify fail"),
        "auth response too large"
    );
}

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn declared_oversize_is_rejected_before_body_arrives() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buf[..n]);
            assert!(request.len() < 16384);
        }
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    EXPECTED_CAP + 1
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        // Do not send any body bytes until the reader has already returned.
        let _ = released.await;
    });
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
        .get(format!("http://{address}/"))
        .send()
        .await
        .unwrap();
    let result = super::token_exchange::read_auth_response::<Value>(response).await;
    let _ = release.send(());
    server.await.unwrap();
    assert_eq!(
        result.unwrap_err(),
        OidcVerifyError::AuthResponse(super::super::error::AuthResponseFailure::TooLarge)
    );
}
