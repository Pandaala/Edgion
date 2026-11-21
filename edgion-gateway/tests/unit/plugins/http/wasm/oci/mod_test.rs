use super::*;
use base64::Engine as _;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WASM: &[u8] = b"\0asm\x01\x00\x00\x00";

fn digest_of(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn image_manifest(layer: &[u8], media_type: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [{"mediaType": media_type, "digest": digest_of(layer), "size": layer.len()}]
    }))
    .unwrap()
}

fn spec_for(server: &MockServer, image_suffix: &str) -> WasmOciSpec {
    let host = server.uri().trim_start_matches("http://").to_string();
    serde_json::from_value(serde_json::json!({ "image": format!("{host}/{image_suffix}") })).unwrap()
}

async fn mount_image(server: &MockServer, reference: &str, manifest: &[u8], blob: &[u8]) {
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/wasm/manifests/{reference}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest.to_vec()))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(blob))))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(blob.to_vec()))
        .mount(server)
        .await;
}

#[tokio::test]
async fn anonymous_pull_oci_variant() {
    let server = MockServer::start().await;
    let manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
    mount_image(&server, "1.0.0", &manifest, WASM).await;
    let fetched = fetch_oci_bytes(&spec_for(&server, "test/wasm:1.0.0")).await.unwrap();
    assert_eq!(fetched.bytes.as_ref(), WASM);
    assert_eq!(fetched.manifest_digest, digest_of(&manifest));
}

#[tokio::test]
async fn compat_variant_extracts_plugin_wasm() {
    let server = MockServer::start().await;
    let layer = {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let enc = GzEncoder::new(Vec::new(), Compression::default());
        let mut b = tar::Builder::new(enc);
        let mut h = tar::Header::new_gnu();
        h.set_size(WASM.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "plugin.wasm", WASM).unwrap();
        b.into_inner().unwrap().finish().unwrap()
    };
    let manifest = image_manifest(&layer, "application/vnd.docker.image.rootfs.diff.tar.gzip");
    mount_image(&server, "compat", &manifest, &layer).await;
    let fetched = fetch_oci_bytes(&spec_for(&server, "test/wasm:compat")).await.unwrap();
    assert_eq!(fetched.bytes.as_ref(), WASM);
}

#[tokio::test]
async fn private_pull_via_bearer_token_flow() {
    let server = MockServer::start().await;
    let manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
    let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, server.uri());
    // Catch-all 401 with the challenge (lower priority).
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/1.0.0"))
        .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
        .with_priority(5)
        .mount(&server)
        .await;
    // Token endpoint requires the Basic credentials from the pull secret.
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("alice:s3cret")
    );
    Mock::given(method("GET"))
        .and(path("/token"))
        .and(query_param("service", "test-registry"))
        .and(header("authorization", basic.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"token": "test-token"})))
        .mount(&server)
        .await;
    // Bearer-authed requests succeed (higher priority than the 401).
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/1.0.0"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest.clone()))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(WASM))))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(WASM.to_vec()))
        .mount(&server)
        .await;

    let host = server.uri().trim_start_matches("http://").to_string();
    let auth_b64 = base64::engine::general_purpose::STANDARD.encode("alice:s3cret");
    let mut spec = spec_for(&server, "test/wasm:1.0.0");
    spec.resolved_pull_secret = Some(format!(r#"{{"auths":{{"{host}":{{"auth":"{auth_b64}"}}}}}}"#));
    let fetched = fetch_oci_bytes(&spec).await.unwrap();
    assert_eq!(fetched.bytes.as_ref(), WASM);
}

#[test]
fn remote_target_policy_covers_origins_and_ip_classes() {
    let registry = Url::parse("http://127.0.0.1:5000").unwrap().origin();
    let permissive = EgressPolicy { block_private: false };
    let strict = EgressPolicy { block_private: true };

    assert_eq!(
        guard_remote_target(&Url::parse("http://127.0.0.1:5000/token").unwrap(), &registry, strict),
        Ok(()),
        "the operator-selected registry origin is always allowed"
    );
    assert_eq!(
        guard_remote_target(
            &Url::parse("http://127.0.0.1:5001/token").unwrap(),
            &registry,
            permissive,
        ),
        Err(RemoteTargetError::Blocked("loopback"))
    );
    assert_eq!(
        guard_remote_target(
            &Url::parse("http://169.254.169.254/token").unwrap(),
            &registry,
            permissive,
        ),
        Err(RemoteTargetError::Blocked("link_local"))
    );
    assert_eq!(
        guard_remote_target(&Url::parse("http://0.0.0.0/token").unwrap(), &registry, permissive),
        Err(RemoteTargetError::Blocked("unspecified"))
    );
    assert_eq!(
        guard_remote_target(&Url::parse("http://224.0.0.1/token").unwrap(), &registry, permissive),
        Err(RemoteTargetError::Blocked("non_dialable"))
    );
    for target in [
        "http://10.0.0.1/token",
        "http://100.64.0.1/token",
        "http://[fd00::1]/token",
    ] {
        let target = Url::parse(target).unwrap();
        assert_eq!(guard_remote_target(&target, &registry, permissive), Ok(()));
        assert_eq!(
            guard_remote_target(&target, &registry, strict),
            Err(RemoteTargetError::Blocked("private"))
        );
    }
    for target in [
        "http://8.8.8.8/token",
        "http://[2606:4700:4700::1111]/token",
        "http://cdn.example/token",
    ] {
        assert_eq!(
            guard_remote_target(&Url::parse(target).unwrap(), &registry, strict),
            Ok(()),
            "global literals and hostnames remain allowed: {target}"
        );
    }
    assert_eq!(
        guard_remote_target(
            &Url::parse("http://[::ffff:127.0.0.1]/token").unwrap(),
            &registry,
            permissive,
        ),
        Err(RemoteTargetError::Blocked("loopback"))
    );
}

#[test]
fn token_realm_transport_policy_preserves_http_registries_and_https_realms() {
    let http_realm = Url::parse("http://auth.example/token").unwrap();
    let https_realm = Url::parse("https://auth.example/token").unwrap();

    assert!(token_realm_downgrades_tls(true, &http_realm));
    assert!(!token_realm_downgrades_tls(false, &http_realm));
    assert!(!token_realm_downgrades_tls(true, &https_realm));
}

#[tokio::test]
async fn https_registry_http_token_realm_is_rejected_before_target_request() {
    let target = MockServer::start().await;
    let spec = spec_for(&target, "test/wasm:1.0.0");
    let image = OciImageRef::parse(&spec.image).unwrap();
    let mut session = RegistrySession::new(&image, &spec).unwrap();
    session.registry_origin = Url::parse("https://registry.example").unwrap().origin();
    session.registry_uses_tls = true;
    session.auth = Some(RegistryAuth {
        username: "alice".to_string(),
        password: "s3cret".to_string(),
    });
    let challenge = BearerChallenge {
        realm: format!("{}/token", target.uri()),
        service: Some("registry.example".to_string()),
        scope: None,
    };

    let error = match session.fetch_token(&challenge).await {
        Err(GetError::Definitive(error)) => error,
        Err(GetError::Retryable(error)) => panic!("downgrade must be definitive, got retryable: {error}"),
        Ok(_) => panic!("downgrade must not return a token"),
    };

    assert_eq!(error, "token endpoint blocked by transport policy");
    assert!(
        target.received_requests().await.unwrap().is_empty(),
        "the plaintext token realm must receive zero credential-bearing requests"
    );
}

#[tokio::test]
async fn cross_origin_unsafe_literal_bearer_realm_is_never_requested() {
    let registry = MockServer::start().await;
    let target = MockServer::start().await;
    let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, target.uri());
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/1.0.0"))
        .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
        .mount(&registry)
        .await;

    let err = fetch_oci_bytes(&spec_for(&registry, "test/wasm:1.0.0"))
        .await
        .unwrap_err();

    assert_eq!(err, "token endpoint blocked by egress policy");
    assert!(
        target.received_requests().await.unwrap().is_empty(),
        "a remote-controlled loopback realm must be rejected before GET"
    );
}

#[tokio::test]
async fn cross_origin_unsafe_literal_blob_redirect_is_never_requested() {
    const REDIRECT_BLOB: &[u8] = b"issue-9-redirect-target";
    let registry = MockServer::start().await;
    let target = MockServer::start().await;
    let manifest = image_manifest(REDIRECT_BLOB, manifest::MEDIA_TYPE_WASM_LAYER);
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/redirect"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest))
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(REDIRECT_BLOB))))
        .respond_with(ResponseTemplate::new(307).insert_header("location", format!("{}/blob", target.uri()).as_str()))
        .mount(&registry)
        .await;

    let err = fetch_oci_bytes(&spec_for(&registry, "test/wasm:redirect"))
        .await
        .unwrap_err();

    assert_eq!(err, "OCI redirect blocked by policy");
    assert!(
        target.received_requests().await.unwrap().is_empty(),
        "a remote-controlled loopback redirect must be rejected before GET"
    );
}

#[tokio::test]
async fn cross_origin_hostname_blob_redirect_is_followed() {
    let registry = MockServer::start().await;
    let target = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/blob"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
        .mount(&target)
        .await;
    let location = format!("http://cdn.test:{}/blob", target.address().port());
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", location.as_str()))
        .mount(&registry)
        .await;

    let registry_origin = Url::parse(&registry.uri()).unwrap().origin();
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve("cdn.test", *target.address())
        .redirect(blob_redirect_policy(
            registry_origin,
            EgressPolicy { block_private: true },
        ))
        .build()
        .unwrap();
    let response = client.get(format!("{}/start", registry.uri())).send().await.unwrap();

    assert_eq!(response.bytes().await.unwrap(), b"ok".as_slice());
    assert_eq!(target.received_requests().await.unwrap().len(), 1);
}

async fn truncated_http_target() -> (
    std::net::SocketAddr,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let server = {
        let connections = connections.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            connections.fetch_add(1, Ordering::AcqRel);
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 64\r\nConnection: close\r\n\r\n{")
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        })
    };
    (address, connections, server)
}

#[tokio::test]
async fn blob_redirect_body_error_redacts_final_url() {
    use std::sync::atomic::Ordering;

    let registry = MockServer::start().await;
    let (target_address, target_connections, target_server) = truncated_http_target().await;
    let secret = "signed-query-secret";
    let location = format!("http://cdn.test:{}/blob?signature={secret}", target_address.port());
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", location.as_str()))
        .expect(1)
        .mount(&registry)
        .await;

    let spec = spec_for(&registry, "test/wasm:redirect-error");
    let image = OciImageRef::parse(&spec.image).unwrap();
    let mut session = RegistrySession::new(&image, &spec).unwrap();
    session.client_blob = reqwest::Client::builder()
        .no_proxy()
        .resolve("cdn.test", target_address)
        .redirect(blob_redirect_policy(
            Url::parse(&registry.uri()).unwrap().origin(),
            EgressPolicy { block_private: true },
        ))
        .build()
        .unwrap();
    let initial_url = format!("{}/start", registry.uri());

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        session.get_authed(true, &initial_url, "application/octet-stream", 64),
    )
    .await
    .expect("redirected blob body read must complete within the test timeout");
    let error = match result {
        Err(GetError::Retryable(error)) => error,
        Err(GetError::Definitive(_)) => panic!("truncated blob body must remain retryable"),
        Ok(_) => panic!("truncated blob body must not succeed"),
    };

    assert!(
        error.starts_with(&format!("read body of {initial_url}: ")),
        "the safe initial registry URL must remain as error context"
    );
    assert!(!error.contains(secret), "the secret query must be redacted");
    assert!(!error.contains("cdn.test"), "the final redirect URL must be redacted");
    tokio::time::timeout(Duration::from_secs(2), target_server)
        .await
        .expect("redirect target fixture must complete within the test timeout")
        .expect("redirect target fixture must join successfully");
    assert_eq!(registry.received_requests().await.unwrap().len(), 1);
    assert_eq!(target_connections.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn token_body_error_redacts_realm_url() {
    use std::sync::atomic::Ordering;

    let registry = MockServer::start().await;
    let (target_address, target_connections, target_server) = truncated_http_target().await;
    let secret = "realm-query-secret";
    let realm = format!("http://auth.test:{}/token?credential={secret}", target_address.port());
    let challenge_header = format!(r#"Bearer realm="{realm}",service="registry.test""#);
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge_header.as_str()))
        .expect(1)
        .mount(&registry)
        .await;

    let spec = spec_for(&registry, "test/wasm:token-error");
    let image = OciImageRef::parse(&spec.image).unwrap();
    let mut session = RegistrySession::new(&image, &spec).unwrap();
    session.client_deny = reqwest::Client::builder()
        .no_proxy()
        .resolve("auth.test", target_address)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let initial_url = format!("{}/start", registry.uri());

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        session.get_authed(false, &initial_url, "application/octet-stream", 16),
    )
    .await
    .expect("Bearer token body read must complete within the test timeout");
    let error = match result {
        Err(GetError::Retryable(error)) => error,
        Err(GetError::Definitive(_)) => panic!("truncated token body must remain retryable"),
        Ok(_) => panic!("truncated token body must not succeed"),
    };

    assert!(
        error.starts_with("token response: "),
        "the token response classification must remain stable"
    );
    assert!(!error.contains(secret), "the secret realm query must be redacted");
    assert!(!error.contains("auth.test"), "the token realm URL must be redacted");
    tokio::time::timeout(Duration::from_secs(2), target_server)
        .await
        .expect("token target fixture must complete within the test timeout")
        .expect("token target fixture must join successfully");
    assert_eq!(registry.received_requests().await.unwrap().len(), 1);
    assert_eq!(target_connections.load(Ordering::Acquire), 1);
}

#[test]
fn blob_redirect_transport_policy_only_rejects_https_to_http() {
    let source_https = Url::parse("https://registry.example/blob").unwrap();
    let source_http = Url::parse("http://registry.example/blob").unwrap();

    assert!(redirect_downgrades_tls(
        std::slice::from_ref(&source_https),
        &Url::parse("http://registry.example/blob-2").unwrap(),
    ));
    for (source, target) in [
        (&source_http, Url::parse("http://registry.example/blob-2").unwrap()),
        (&source_http, Url::parse("https://registry.example/blob-2").unwrap()),
        (&source_https, Url::parse("https://registry.example/blob-2").unwrap()),
    ] {
        assert!(
            !redirect_downgrades_tls(std::slice::from_ref(source), &target),
            "non-downgrade redirect must remain allowed: {source} -> {target}"
        );
    }
}

#[tokio::test]
async fn authorized_https_blob_redirect_to_http_is_rejected_before_target_request() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let _ = rustls::crypto::ring::default_provider().install_default();
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certified.cert.der().clone()], key.into())
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let saw_authorization = Arc::new(AtomicBool::new(false));
    let plaintext_requests = Arc::new(AtomicUsize::new(0));

    let server = {
        let saw_authorization = saw_authorization.clone();
        let plaintext_requests = plaintext_requests.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(stream).await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = tls.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
            }
            let request = String::from_utf8_lossy(&request);
            saw_authorization.store(
                request
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token")),
                Ordering::Release,
            );
            let response = format!(
                "HTTP/1.1 307 Temporary Redirect\r\n\
                     Location: http://{address}/plaintext-target\r\n\
                     Content-Length: 0\r\n\
                     Connection: close\r\n\r\n"
            );
            tls.write_all(response.as_bytes()).await.unwrap();
            tls.shutdown().await.unwrap();
            drop(tls);

            if let Ok(Ok((_stream, _))) = tokio::time::timeout(Duration::from_millis(500), listener.accept()).await {
                plaintext_requests.fetch_add(1, Ordering::AcqRel);
            }
        })
    };

    let source = Url::parse(&format!("https://{address}/blob")).unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .redirect(blob_redirect_policy(
            source.origin(),
            EgressPolicy { block_private: false },
        ))
        .build()
        .unwrap();
    let error = client.get(source).bearer_auth("test-token").send().await.unwrap_err();

    assert!(error.is_redirect(), "downgrade must fail in the redirect policy");
    server.await.unwrap();
    assert!(
        saw_authorization.load(Ordering::Acquire),
        "the source request must carry Authorization so the leak boundary is exercised"
    );
    assert_eq!(
        plaintext_requests.load(Ordering::Acquire),
        0,
        "the plaintext downgrade target must receive zero requests"
    );
}

#[tokio::test]
async fn blob_redirect_policy_allows_exactly_three_hops() {
    let server = MockServer::start().await;
    for (from, to) in [
        ("/three-0", "/three-1"),
        ("/three-1", "/three-2"),
        ("/three-2", "/three-3"),
    ] {
        Mock::given(method("GET"))
            .and(path(from))
            .respond_with(ResponseTemplate::new(307).insert_header("location", to))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/three-3"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let spec = spec_for(&server, "test/wasm:redirect-limit");
    let image = OciImageRef::parse(&spec.image).unwrap();
    let mut session = RegistrySession::new(&image, &spec).unwrap();
    let body = session
        .get(
            true,
            &format!("{}/three-0", server.uri()),
            "application/octet-stream",
            16,
        )
        .await
        .unwrap();

    assert_eq!(body, b"ok");
}

#[tokio::test]
async fn blob_redirect_policy_rejects_fourth_hop_before_target_request() {
    let server = MockServer::start().await;
    for (from, to) in [
        ("/four-0", "/four-1"),
        ("/four-1", "/four-2"),
        ("/four-2", "/four-3"),
        ("/four-3", "/four-never"),
    ] {
        Mock::given(method("GET"))
            .and(path(from))
            .respond_with(ResponseTemplate::new(307).insert_header("location", to))
            .expect(1)
            .mount(&server)
            .await;
    }

    let spec = spec_for(&server, "test/wasm:redirect-limit");
    let image = OciImageRef::parse(&spec.image).unwrap();
    let mut session = RegistrySession::new(&image, &spec).unwrap();
    let err = session
        .get(
            true,
            &format!("{}/four-0", server.uri()),
            "application/octet-stream",
            16,
        )
        .await
        .unwrap_err();

    assert_eq!(err, "OCI redirect blocked by policy");
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.url.path() != "/four-never"),
        "the fourth redirect target must not be requested"
    );
}

#[tokio::test]
async fn token_endpoint_5xx_retries_and_then_succeeds() {
    let server = MockServer::start().await;
    let manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
    let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, server.uri());
    // Catch-all 401 with the challenge (lower priority).
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/1.0.0"))
        .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
        .with_priority(5)
        .mount(&server)
        .await;
    // First token exchange returns 503 (transient); it must be retried.
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("alice:s3cret")
    );
    Mock::given(method("GET"))
        .and(path("/token"))
        .and(query_param("service", "test-registry"))
        .and(header("authorization", basic.as_str()))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    // Subsequent token exchange succeeds.
    Mock::given(method("GET"))
        .and(path("/token"))
        .and(query_param("service", "test-registry"))
        .and(header("authorization", basic.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"token": "test-token"})))
        .with_priority(5)
        .mount(&server)
        .await;
    // Bearer-authed requests succeed (higher priority than the 401).
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/1.0.0"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest.clone()))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(WASM))))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(WASM.to_vec()))
        .mount(&server)
        .await;

    let host = server.uri().trim_start_matches("http://").to_string();
    let auth_b64 = base64::engine::general_purpose::STANDARD.encode("alice:s3cret");
    let mut spec = spec_for(&server, "test/wasm:1.0.0");
    spec.resolved_pull_secret = Some(format!(r#"{{"auths":{{"{host}":{{"auth":"{auth_b64}"}}}}}}"#));
    let fetched = fetch_oci_bytes(&spec).await.unwrap();
    assert_eq!(fetched.bytes.as_ref(), WASM);
}

#[tokio::test]
async fn token_endpoint_403_is_definitive_no_retry() {
    let server = MockServer::start().await;
    let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, server.uri());
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/1.0.0"))
        .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
        .with_priority(5)
        .mount(&server)
        .await;
    // Token endpoint rejects the credentials; a 403 must stop after one attempt.
    Mock::given(method("GET"))
        .and(path("/token"))
        .and(query_param("service", "test-registry"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;

    let host = server.uri().trim_start_matches("http://").to_string();
    let auth_b64 = base64::engine::general_purpose::STANDARD.encode("alice:s3cret");
    let mut spec = spec_for(&server, "test/wasm:1.0.0");
    spec.resolved_pull_secret = Some(format!(r#"{{"auths":{{"{host}":{{"auth":"{auth_b64}"}}}}}}"#));
    let err = fetch_oci_bytes(&spec).await.unwrap_err();
    assert!(err.contains("token endpoint: status 403"), "got: {err}");
}

#[tokio::test]
async fn retry_backoff_releases_permit_for_another_registry_request() {
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Notify, Semaphore};

    let server = MockServer::start().await;
    let order = Arc::new(Mutex::new(Vec::new()));
    let first_a_seen = Arc::new(Notify::new());
    let a_calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with({
            let order = order.clone();
            let first_a_seen = first_a_seen.clone();
            let a_calls = a_calls.clone();
            move |_: &wiremock::Request| {
                let call = a_calls.fetch_add(1, Ordering::SeqCst);
                order.lock().push(if call == 0 { "a1" } else { "a2" });
                if call == 0 {
                    first_a_seen.notify_one();
                    ResponseTemplate::new(503)
                } else {
                    ResponseTemplate::new(200).set_body_bytes(b"a".to_vec())
                }
            }
        })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with({
            let order = order.clone();
            move |_: &wiremock::Request| {
                order.lock().push("b");
                ResponseTemplate::new(200).set_body_bytes(b"b".to_vec())
            }
        })
        .mount(&server)
        .await;

    let spec = spec_for(&server, "test/wasm:1.0");
    let image = OciImageRef::parse(&spec.image).unwrap();
    let mut session_a = RegistrySession::new(&image, &spec).unwrap();
    let mut session_b = RegistrySession::new(&image, &spec).unwrap();
    let permits = Arc::new(Semaphore::new(1));
    let url_a = format!("{}/a", server.uri());
    let url_b = format!("{}/b", server.uri());

    let permits_a = permits.clone();
    let a = tokio::spawn(async move {
        session_a
            .get_with_permits(&permits_a, false, &url_a, "application/octet-stream", 16)
            .await
    });
    first_a_seen.notified().await;
    let permits_b = permits.clone();
    let b = tokio::spawn(async move {
        session_b
            .get_with_permits(&permits_b, false, &url_b, "application/octet-stream", 16)
            .await
    });

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), b)
            .await
            .expect("B completes while A is backing off")
            .unwrap()
            .unwrap(),
        b"b"
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), a)
            .await
            .expect("A completes its retry")
            .unwrap()
            .unwrap(),
        b"a"
    );
    assert_eq!(&*order.lock(), &["a1", "b", "a2"]);
}

#[tokio::test]
async fn declared_but_unresolved_pull_secret_fails_closed() {
    let server = MockServer::start().await;
    let mut spec = spec_for(&server, "test/wasm:1.0.0");
    spec = serde_json::from_value(serde_json::json!({
        "image": spec.image,
        "pullSecretRef": {"name": "creds"}
    }))
    .unwrap();
    let err = fetch_oci_bytes(&spec).await.unwrap_err();
    assert!(err.contains("pull secret not resolved"), "got: {err}");
}

#[tokio::test]
async fn blob_digest_mismatch_is_a_hard_failure() {
    let server = MockServer::start().await;
    let manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
    // Serve WRONG bytes under the manifest's layer digest.
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/1.0.0"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(WASM))))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"evil".to_vec()))
        .mount(&server)
        .await;
    let err = fetch_oci_bytes(&spec_for(&server, "test/wasm:1.0.0"))
        .await
        .unwrap_err();
    assert!(err.contains("digest mismatch"), "got: {err}");
}

#[tokio::test]
async fn manifest_404_is_definitive_no_retry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/missing"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1) // definitive: exactly one attempt
        .mount(&server)
        .await;
    let err = fetch_oci_bytes(&spec_for(&server, "test/wasm:missing"))
        .await
        .unwrap_err();
    assert!(err.contains("status 404"), "got: {err}");
}

#[tokio::test]
async fn index_resolves_to_first_manifest() {
    let server = MockServer::start().await;
    let img_manifest = image_manifest(WASM, manifest::MEDIA_TYPE_WASM_LAYER);
    let index = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "mediaType": manifest::MEDIA_TYPE_OCI_INDEX,
        "manifests": [{"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": digest_of(&img_manifest)}]
    }))
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/multi"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(index.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/wasm/manifests/{}", digest_of(&img_manifest))))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(img_manifest.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(WASM))))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(WASM.to_vec()))
        .mount(&server)
        .await;
    let fetched = fetch_oci_bytes(&spec_for(&server, "test/wasm:multi")).await.unwrap();
    assert_eq!(fetched.bytes.as_ref(), WASM);
    assert_eq!(fetched.manifest_digest, digest_of(&index));

    // The Always re-check must agree with the stored identity, or an
    // index-backed image would reload on every re-check window.
    let recheck = resolve_manifest_digest(&spec_for(&server, "test/wasm:multi"))
        .await
        .unwrap();
    assert_eq!(recheck, fetched.manifest_digest);
}

/// An oversized manifest body is rejected by the streaming cap. Because the
/// helper counts actually-streamed bytes (not `Content-Length`), the same
/// rejection holds for an absent, false-small, or chunked Content-Length.
#[tokio::test]
async fn oversized_manifest_is_rejected_at_the_cap() {
    let server = MockServer::start().await;
    let big = vec![b'x'; MAX_MANIFEST_BYTES + 1];
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/big"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(big))
        .mount(&server)
        .await;
    let err = fetch_oci_bytes(&spec_for(&server, "test/wasm:big")).await.unwrap_err();
    assert!(err.contains("body exceeds"), "got: {err}");
}

/// An oversized token-endpoint body is rejected by the streaming cap rather
/// than buffered whole by `resp.json()`.
#[tokio::test]
async fn oversized_token_response_is_rejected_at_the_cap() {
    let server = MockServer::start().await;
    let challenge = format!(r#"Bearer realm="{}/token",service="test-registry""#, server.uri());
    Mock::given(method("GET"))
        .and(path("/v2/test/wasm/manifests/1.0.0"))
        .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
        .mount(&server)
        .await;
    let big = vec![b'x'; MAX_TOKEN_BYTES + 1];
    Mock::given(method("GET"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(big))
        .mount(&server)
        .await;
    let host = server.uri().trim_start_matches("http://").to_string();
    let auth_b64 = base64::engine::general_purpose::STANDARD.encode("alice:s3cret");
    let mut spec = spec_for(&server, "test/wasm:1.0.0");
    spec.resolved_pull_secret = Some(format!(r#"{{"auths":{{"{host}":{{"auth":"{auth_b64}"}}}}}}"#));
    let err = fetch_oci_bytes(&spec).await.unwrap_err();
    assert!(err.contains("response exceeds"), "got: {err}");
}

async fn fetch_oci_bytes(spec: &WasmOciSpec) -> Result<OciFetched, String> {
    super::fetch_oci_bytes(spec, crate::plugins::http::wasm::admission::try_prepare()?).await
}

#[test]
fn configured_cidr_rejects_initial_registry_origin() {
    use crate::test_blocked_cidr::{install, BLOCKED_CIDR_TEST_IP};
    install();
    let spec: WasmOciSpec = serde_json::from_value(serde_json::json!({
        "image": format!("{BLOCKED_CIDR_TEST_IP}/test/module:v1")
    }))
    .unwrap();
    let image = OciImageRef::parse(&spec.image).unwrap();
    assert_eq!(
        RegistrySession::new(&image, &spec).err().as_deref(),
        Some("Wasm destination blocked by configured egress CIDR")
    );
}
