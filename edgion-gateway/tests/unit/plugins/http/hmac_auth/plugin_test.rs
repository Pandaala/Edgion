use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use bytes::Bytes;
use mockall::Sequence;

#[test]
fn credential_debug_omits_identity_and_signature() {
    let params = HmacAuthParams {
        username: "private-identity".to_string(),
        algorithm: HmacAlgorithm::HmacSha256,
        headers: vec!["private-header".to_string()],
        signature: b"private-signature".to_vec(),
    };
    let outcome = HmacVerifyOutcome::Verified {
        username: params.username.clone(),
    };
    for value in [&params as &dyn std::fmt::Debug, &outcome] {
        assert!(!format!("{value:?}").contains("private-"));
        assert!(!format!("{value:#?}").contains("private-"));
        assert!(!format!("{:?}", Some(value)).contains("private-"));
    }
    assert!(!format!("{params:?}").contains("112, 114"));
    assert_eq!(params.signature, b"private-signature");
    assert!(matches!(outcome, HmacVerifyOutcome::Verified { username } if username == "private-identity"));
}

fn create_plugin() -> HmacAuth {
    let mut config = HmacAuthConfig {
        match_config: HmacMatchConfig {
            clock_skew: 600,
            enforce_headers: Some(vec!["@request-target".to_string()]),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut credentials = HashMap::new();
    credentials.insert(
        "alice".to_string(),
        HmacCredential {
            secret: b"alice-super-secret-key".to_vec(),
            headers: HashMap::from([("X-Consumer-Username".to_string(), "alice".to_string())]),
            group: "default".to_string(),
        },
    );
    config.resolved_credentials = Some(credentials);

    HmacAuth::new(&config)
}

#[test]
fn constructor_retains_only_live_policy_and_one_verifier_generation() {
    let (plugin, source_secret_ptr) = {
        let mut config = HmacAuthConfig {
            auth_failure_delay_ms: 17,
            realm: "partners".to_string(),
            anonymous: Some("guest".to_string()),
            hide_credentials: true,
            ..Default::default()
        };
        config.resolved_credentials = Some(HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: b"alice-super-secret-key".to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        )]));
        let source_secret_ptr = config
            .resolved_credentials
            .as_ref()
            .and_then(|credentials| credentials.get("alice"))
            .expect("test credential must exist")
            .secret
            .as_ptr();
        (HmacAuth::new(&config), source_secret_ptr)
    };

    assert_eq!(plugin.auth_failure_delay_ms, 17);
    assert_eq!(plugin.realm, "partners");
    assert_eq!(plugin.anonymous.as_deref(), Some("guest"));
    assert!(plugin.hide_credentials);
    let credential = plugin
        .verifier
        .credential("alice")
        .expect("verifier credential must exist");
    assert_eq!(credential.secret, b"alice-super-secret-key");
    assert_ne!(credential.secret.as_ptr(), source_secret_ptr);
}

fn rfc2822_now() -> String {
    Utc::now().to_rfc2822()
}

#[test]
fn test_build_signing_string_request_target() {
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/v1/items".to_string());
    session.expect_request_query().return_const(Some("page=1"));
    session.expect_get_method().return_const("GET".to_string());

    let signing = HmacAuth::build_signing_string(&session, &[String::from("@request-target")]).unwrap();
    assert_eq!(signing, "@request-target: get /v1/items?page=1");
}

#[test]
fn canonical_signing_and_hmac_vectors_are_wire_stable() {
    let mut session = MockPluginSession::new();
    session.expect_get_path().times(2).return_const("/v1/items".to_string());
    session
        .expect_request_query()
        .times(2)
        .return_const(Some("page=1&tag=a%20b"));
    session.expect_get_method().times(2).return_const("PoSt".to_string());
    setup_header_mocks(
        &mut session,
        &[("host", "API.Example:8443"), ("x-custom", "MiXeD/Value==;")],
    );
    let headers = ["@request-target", "host", "x-custom", "(request-target)"]
        .map(str::to_string)
        .to_vec();
    let expected = concat!(
        "@request-target: post /v1/items?page=1&tag=a%20b\n",
        "host: API.Example:8443\n",
        "x-custom: MiXeD/Value==;\n",
        "(request-target): post /v1/items?page=1&tag=a%20b"
    );

    let signing = HmacAuth::build_signing_string(&session, &headers).expect("canonical signing must succeed");

    assert_eq!(signing, expected);
    assert_eq!(signing.len(), 146);
    for (algorithm, expected_signature) in [
        (
            HmacAlgorithm::HmacSha256,
            "Zfsfak+e6CMopCzcmNeHtfKGtHH+ksXcWtI/O8qn1+A=",
        ),
        (
            HmacAlgorithm::HmacSha384,
            "UyQwM5enk4xAyaGZCW/HhOkMxj8rAvb3EfDbHbpMZMapN1hYuSs5H/iEmqpQ/9O6",
        ),
        (
            HmacAlgorithm::HmacSha512,
            "sIFwUh6tTKGSXsCoE9LqRQKlY/3Zj+Hwjat+YXYwH8xw5j/j+hhqDzqdgVUwNXcJ1IzSitgN7wvbtKYr7tB5Kw==",
        ),
    ] {
        let signature = HmacAuth::compute_hmac(algorithm, b"correct horse battery staple", &signing)
            .expect("test HMAC must succeed");
        assert_eq!(STANDARD.encode(&signature), expected_signature);
        assert!(HmacAuth::verify_signature(
            algorithm,
            b"correct horse battery staple",
            &signing,
            &signature,
        ));
    }
}

#[test]
fn test_check_body_digest_matches() {
    let body = b"{\"hello\":\"world\"}";
    let mut hasher = Sha256::new();
    hasher.update(body);
    let digest = format!("SHA-256={}", STANDARD.encode(hasher.finalize()));
    assert!(HmacAuth::check_body_digest(body, &digest).is_ok());
}

#[test]
fn test_check_body_digest_mismatch() {
    let body = b"tampered";
    // Digest computed over different bytes.
    let mut hasher = Sha256::new();
    hasher.update(b"original");
    let digest = format!("SHA-256={}", STANDARD.encode(hasher.finalize()));
    assert_eq!(HmacAuth::check_body_digest(body, &digest), Err("Body digest mismatch"));
}

#[test]
fn test_check_empty_body_digest_matches() {
    let digest = format!("SHA-256={}", STANDARD.encode(Sha256::digest([])));
    assert!(HmacAuth::check_body_digest(b"", &digest).is_ok());
}

#[test]
fn test_check_empty_body_digest_mismatch() {
    let digest = format!("SHA-256={}", STANDARD.encode(Sha256::digest(b"not empty")));
    assert_eq!(HmacAuth::check_body_digest(b"", &digest), Err("Body digest mismatch"));
}

#[test]
fn test_check_body_digest_bad_algorithm_and_format() {
    assert_eq!(
        HmacAuth::check_body_digest(b"x", "MD5=abc"),
        Err("Unsupported Digest algorithm")
    );
    assert_eq!(
        HmacAuth::check_body_digest(b"x", "no-equals-sign"),
        Err("Malformed Digest header")
    );
}

/// Every distinct `BodyReadError` must reach the access log as its own
/// `EdgionErrorCode`, matching `body_gate.rs` / `wasm/plugin.rs`. The denial
/// reason returned to the caller stays opaque, so `record_error` is the only
/// place the cause is observable — collapsing the variants here would make
/// a timeout indistinguishable from an oversize body in production.
#[tokio::test]
async fn verify_body_digest_records_status_per_read_error() {
    for (err, expected) in [
        (BodyReadError::TooLarge { limit: 8 }, EdgionErrorCode::BodyReadTooLarge),
        (BodyReadError::Timeout, EdgionErrorCode::BodyReadTimeout),
        (BodyReadError::Incomplete, EdgionErrorCode::BodyReadAborted),
        (BodyReadError::Unimplemented, EdgionErrorCode::BodyReadUnimplemented),
    ] {
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("digest", "SHA-256=unused")]);
        session.expect_request_body_sha256().returning(move || Err(err.clone()));
        session
            .expect_record_error()
            .withf(move |status| *status == expected)
            .times(1)
            .returning(|_| ());

        assert_eq!(
            HmacAuth::verify_body_digest(&mut session).await,
            Err("request body unavailable")
        );
    }
}

/// `NotBuffered` is the one variant that must NOT be re-recorded: the read
/// adapter already put the specific refusal on ctx.
#[tokio::test]
async fn verify_body_digest_does_not_double_count_refusal() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("digest", "SHA-256=unused")]);
    session
        .expect_request_body_sha256()
        .returning(|| Err(BodyReadError::NotBuffered));
    session.expect_record_error().times(0);

    assert_eq!(
        HmacAuth::verify_body_digest(&mut session).await,
        Err("request body unavailable")
    );
}

#[test]
fn test_body_requirement_declared_only_when_validate_body() {
    use edgion_resources::resources::edgion_plugins::EdgionPlugin;
    let base = HmacAuthConfig {
        match_config: HmacMatchConfig {
            resolved_credentials: Some(HashMap::new()),
            ..Default::default()
        },
        ..Default::default()
    };
    use edgion_resources::resources::edgion_plugins::BodyStage;
    assert!(EdgionPlugin::HmacAuth(base.clone())
        .body_requirement(BodyStage::Request)
        .is_none());
    assert!(!EdgionPlugin::HmacAuth(base.clone()).accepts_body(BodyStage::Request));

    let with_body = HmacAuthConfig {
        match_config: HmacMatchConfig {
            validate_request_body: true,
            ..base.match_config.clone()
        },
        ..base
    };
    // request-only plugin: consumes at the request stage
    assert!(EdgionPlugin::HmacAuth(with_body.clone())
        .body_requirement(BodyStage::Request)
        .is_some());
    assert!(EdgionPlugin::HmacAuth(with_body.clone())
        .body_requirement(BodyStage::Response)
        .is_none());
    assert!(EdgionPlugin::HmacAuth(with_body.clone()).accepts_body(BodyStage::Request));
    assert!(!EdgionPlugin::HmacAuth(with_body).accepts_body(BodyStage::Response));
}

#[test]
fn test_verify_signature_success() {
    let secret = b"test-secret";
    let signing = "@request-target: get /ping";
    let digest = HmacAuth::compute_hmac(HmacAlgorithm::HmacSha256, secret, signing).unwrap();
    assert!(HmacAuth::verify_signature(
        HmacAlgorithm::HmacSha256,
        secret,
        signing,
        &digest
    ));
}

#[tokio::test]
async fn test_run_request_rejects_when_date_not_signed() {
    let plugin = create_plugin();
    let date = rfc2822_now();
    let auth_header =
        r#"hmac username="alice", algorithm="hmac-sha256", headers="@request-target", signature="YmFkLXNpZw==""#
            .to_string();

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("HmacAuth");

    setup_header_mocks(
        &mut session,
        &[("authorization", auth_header.as_str()), ("date", date.as_str())],
    );
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("Deny missing date signed"));
}

#[tokio::test]
async fn no_credentials_uses_generic_exact_internal_response() {
    let config = HmacAuthConfig {
        realm: "partners".to_string(),
        auth_failure_delay_ms: 0,
        ..Default::default()
    };
    let plugin = HmacAuth::new(&config);
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", "hmac unused")]);

    let mut sequence = Sequence::new();
    session
        .expect_write_response_header()
        .withf(|response, end_of_stream| {
            !end_of_stream
                && response.status.as_u16() == 500
                && response
                    .headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    == Some("text/plain")
                && response
                    .headers
                    .get("www-authenticate")
                    .and_then(|value| value.to_str().ok())
                    == Some("hmac realm=\"partners\"")
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_write_response_body()
        .withf(|body, end_of_stream| {
            *end_of_stream && body.as_deref() == Some(Bytes::from_static(b"500 Internal gateway error").as_ref())
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|| {});

    let mut log = PluginLog::new("HmacAuth");
    let result = plugin.run_request(&mut session, &mut log).await;

    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert_eq!(log.render_for_test(), "err: no creds configured");
}

use crate::plugins::runtime::PluginLog;
use serde_json::Value;

fn render_log(log: &PluginLog) -> Vec<String> {
    let v: Value = serde_json::to_value(log).unwrap();
    v.get("log")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

#[test]
fn hmac_hit_log_contains_group_and_user() {
    let mut log = PluginLog::new("HmacAuth");
    let credential = HmacCredential {
        secret: b"a-secret-key-at-least-32-bytes-long".to_vec(),
        headers: HashMap::new(),
        group: "partner-a".to_string(),
    };
    let username = "alice";

    log.push_fmt(
        "OK group={} user {}",
        la![credential.group.as_str(), sensitive(username)],
    );

    let entries = render_log(&log);
    assert!(
        entries
            .iter()
            .any(|e| e.starts_with("OK group=partner-a") && e.contains("user ")),
        "expected 'OK group=partner-a user ...', got: {:?}",
        entries
    );
}

// The "group=" tag in the OK log path is enforced by code review, not by a
// unit test: driving all failure branches here would be pure tautology.

// ===== validate_request_body (body-digest) end-to-end =====

/// Build a mock session carrying a VALID HMAC signature over `signed_headers`
/// for user "alice", plus the given `digest` header and buffered `body`.
/// `signed_headers` are the lowercased names (e.g. `["@request-target","date","digest"]`).
fn signed_session(signed_headers: &[&str], digest: Option<&str>, body: &'static [u8]) -> MockPluginSession {
    let mut session = signed_session_without_digest_read(signed_headers, digest);
    session
        .expect_request_body_sha256()
        .returning(move || Ok(Sha256::digest(body).into()));
    session
}

fn signed_session_without_digest_read(signed_headers: &[&str], digest: Option<&str>) -> MockPluginSession {
    let secret = b"alice-super-secret-key".to_vec();
    let date = rfc2822_now();
    let path = "/v1/items";
    let method = "post";

    // Reconstruct the exact signing string build_signing_string would produce.
    let digest_val = digest.map(|d| d.to_string());
    let mut lines = Vec::new();
    for h in signed_headers {
        match *h {
            "@request-target" => lines.push(format!("@request-target: {method} {path}")),
            "date" => lines.push(format!("date: {date}")),
            "digest" => lines.push(format!("digest: {}", digest_val.clone().unwrap_or_default())),
            other => panic!("unhandled signed header in test: {other}"),
        }
    }
    let signing = lines.join("\n");
    let sig = HmacAuth::compute_hmac(HmacAlgorithm::HmacSha256, &secret, &signing).unwrap();
    let sig_b64 = STANDARD.encode(sig);
    let headers_str = signed_headers.join(" ");
    let auth =
        format!(r#"hmac username="alice", algorithm="hmac-sha256", headers="{headers_str}", signature="{sig_b64}""#);

    let mut session = MockPluginSession::new();
    let mut header_pairs: Vec<(&str, &str)> = vec![("authorization", auth.as_str()), ("date", date.as_str())];
    if let Some(digest_val) = digest_val.as_deref() {
        header_pairs.push(("digest", digest_val));
    }
    setup_header_mocks(&mut session, &header_pairs);
    session.expect_get_path().return_const(path.to_string());
    session.expect_request_query().return_const(None);
    session.expect_get_method().return_const("POST".to_string());
    session.expect_set_request_header_value().returning(|_, value| {
        assert!(value.is_sensitive());
        Ok(())
    });
    session.expect_remove_request_header().returning(|_| Ok(()));
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});
    session
}

#[test]
fn enforce_headers_are_prepared_once_in_order_with_configured_error_spelling() {
    let mut config = HmacMatchConfig {
        clock_skew: 600,
        enforce_headers: Some(vec!["X-Custom".to_string(), "X-Custom".to_string(), "Date".to_string()]),
        ..Default::default()
    };
    config.resolved_credentials = Some(HashMap::from([(
        "alice".to_string(),
        HmacCredential {
            secret: b"alice-super-secret-key".to_vec(),
            headers: HashMap::new(),
            group: "default".to_string(),
        },
    )]));
    let verifier = HmacVerifier::new(&config, None);
    let prepared = verifier
        .enforce_headers
        .as_deref()
        .expect("enforced headers must be prepared");
    assert_eq!(prepared.len(), 3);
    assert_eq!(prepared[0].canonical, "x-custom");
    assert_eq!(prepared[0].configured, "X-Custom");
    assert_eq!(prepared[1].canonical, "x-custom");
    assert_eq!(prepared[1].configured, "X-Custom");
    assert_eq!(prepared[2].canonical, "date");
    assert_eq!(prepared[2].configured, "Date");

    let session = signed_session(&["@request-target", "date"], None, b"");
    match verifier.verify_headers(&session) {
        HmacVerifyOutcome::NoMatch(HmacVerifyFailure::MissingRequiredHeader(message)) => {
            assert_eq!(message, "Missing required signed header: X-Custom");
        }
        outcome => panic!("expected first configured header to be missing, got {outcome:?}"),
    }
}

fn plugin_validate_body() -> HmacAuth {
    plugin_validate_body_with_requirement(None)
}

fn plugin_validate_body_with_requirement(body_requirement: Option<BodyRequirement>) -> HmacAuth {
    let mut config = HmacAuthConfig {
        match_config: HmacMatchConfig {
            clock_skew: 600,
            validate_request_body: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut credentials = HashMap::new();
    credentials.insert(
        "alice".to_string(),
        HmacCredential {
            secret: b"alice-super-secret-key".to_vec(),
            headers: HashMap::new(),
            group: "default".to_string(),
        },
    );
    config.resolved_credentials = Some(credentials);
    HmacAuth::with_body_requirement(&config, body_requirement)
}

fn body_digest(body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(body);
    format!("SHA-256={}", STANDARD.encode(h.finalize()))
}

#[tokio::test]
async fn verify_headers_agrees_with_verify_on_header_only_request() {
    let plugin = create_plugin();
    let mut session = signed_session(&["@request-target", "date"], None, b"");
    let header_outcome = plugin.verifier.verify_headers(&session);
    let verify_outcome = plugin.verifier.verify(&mut session).await;
    match (header_outcome, verify_outcome) {
        (HmacVerifyOutcome::Verified { username: headers }, HmacVerifyOutcome::Verified { username: verify }) => {
            assert_eq!(headers, verify);
            assert_eq!(headers, "alice");
        }
        (headers, verify) => panic!("expected both Verified, got headers={headers:?} verify={verify:?}"),
    }
}

#[tokio::test]
async fn validate_body_rejects_when_digest_not_signed() {
    // F1: digest is NOT in the signed-headers list → must reject even though
    // the signature itself is valid and a (spoofable) Digest header is present.
    let body = b"{\"a\":1}";
    let plugin = plugin_validate_body();
    let mut session = signed_session(&["@request-target", "date"], Some(&body_digest(body)), body);
    session.expect_ensure_request_body().times(0);
    let mut log = PluginLog::new("HmacAuth");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("Deny digest not signed"));
}

#[tokio::test]
async fn validate_body_accepts_matching_signed_digest() {
    let body = b"{\"a\":1}";
    let plugin = plugin_validate_body();
    let mut session = signed_session(&["@request-target", "date", "digest"], Some(&body_digest(body)), body);
    session
        .expect_ensure_request_body()
        .returning(move |_| Ok(body.len() as u64));
    let mut log = PluginLog::new("HmacAuth");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn validate_body_rejects_tampered_body() {
    // Digest is signed and present, but the actual body doesn't match it.
    let real_body = b"{\"a\":1}";
    let plugin = plugin_validate_body();
    // Sign a digest computed over DIFFERENT bytes than the buffered body.
    let signed_digest = body_digest(b"{\"a\":2}");
    let mut session = signed_session(&["@request-target", "date", "digest"], Some(&signed_digest), real_body);
    session
        .expect_ensure_request_body()
        .returning(move |_| Ok(real_body.len() as u64));
    let mut log = PluginLog::new("HmacAuth");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("Deny body digest"));
}

#[tokio::test]
async fn validate_body_enforces_plugin_limit_after_capture() {
    let body = b"12345";
    let plugin = plugin_validate_body_with_requirement(Some(BodyRequirement {
        max_body_size: Some("4".to_string()),
        ..BodyRequirement::fail_close()
    }));
    let mut session =
        signed_session_without_digest_read(&["@request-target", "date", "digest"], Some(&body_digest(body)));
    session
        .expect_ensure_request_body()
        .withf(Option::is_none)
        .times(1)
        .returning(move |_| Ok(body.len() as u64));
    session.expect_request_body_sha256().times(0);
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::BodyReadTooLarge)
        .times(1)
        .returning(|_| ());

    let mut log = PluginLog::new("HmacAuth");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

#[tokio::test]
async fn validate_body_preserves_physical_cap_failure() {
    let body = b"12345";
    let plugin = plugin_validate_body();
    let mut session = signed_session(&["@request-target", "date", "digest"], Some(&body_digest(body)), body);
    session
        .expect_ensure_request_body()
        .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::BodyReadTooLarge)
        .times(1)
        .returning(|_| ());

    let mut log = PluginLog::new("HmacAuth");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

#[tokio::test]
async fn validate_body_fails_closed_without_buffering_capability() {
    let body = b"12345";
    let plugin = plugin_validate_body();
    let mut session = signed_session(&["@request-target", "date", "digest"], Some(&body_digest(body)), body);
    session
        .expect_ensure_request_body()
        .returning(|_| Err(BodyReadError::Unimplemented));
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::BodyReadUnimplemented)
        .times(1)
        .returning(|_| ());

    let mut log = PluginLog::new("HmacAuth");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

#[tokio::test]
async fn validate_body_allows_exact_resolved_limit() {
    let body = b"12345";
    let plugin = plugin_validate_body_with_requirement(Some(BodyRequirement {
        max_body_size: Some("5".to_string()),
        ..BodyRequirement::fail_close()
    }));
    let mut session = signed_session(&["@request-target", "date", "digest"], Some(&body_digest(body)), body);
    session
        .expect_ensure_request_body()
        .withf(Option::is_none)
        .times(1)
        .returning(move |_| Ok(body.len() as u64));
    let mut log = PluginLog::new("HmacAuth");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn invalid_direct_constructor_limits_preserve_intrinsic_fail_close() {
    use edgion_resources::resources::edgion_plugins::conditions::PluginConditions;
    use edgion_resources::resources::edgion_plugins::BodyReadFailureMode;

    for limit in ["invalid", "0"] {
        let plugin = plugin_validate_body_with_requirement(Some(BodyRequirement {
            max_body_size: Some(limit.to_string()),
            on_read_failure: BodyReadFailureMode::FailOpen,
            conditions: Some(PluginConditions::default()),
        }));
        assert_eq!(plugin.verifier.logical_limit_bytes, None);
        assert!(plugin.verifier.body_requirement.is_fail_close());
        assert!(plugin.verifier.body_requirement.conditions.is_none());
        let body = b"12345";
        let mut session =
            signed_session_without_digest_read(&["@request-target", "date", "digest"], Some(&body_digest(body)));
        session
            .expect_ensure_request_body()
            .withf(Option::is_none)
            .times(1)
            .returning(|_| Err(BodyReadError::Timeout));
        session.expect_request_body_sha256().times(0);
        session
            .expect_record_error()
            .withf(|status| *status == EdgionErrorCode::BodyReadTimeout)
            .times(1)
            .returning(|_| ());
        let mut log = PluginLog::new("HmacAuth");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 408,
                body: None
            }
        );
    }
}

#[test]
fn rebuilt_hmac_verifier_resolves_the_new_generation_limit() {
    let original = plugin_validate_body_with_requirement(Some(BodyRequirement {
        max_body_size: Some("1k".to_string()),
        ..BodyRequirement::fail_close()
    }));
    let replacement = plugin_validate_body_with_requirement(Some(BodyRequirement {
        max_body_size: Some("2K".to_string()),
        ..BodyRequirement::fail_close()
    }));
    assert_eq!(original.verifier.logical_limit_bytes, Some(1024));
    assert_eq!(replacement.verifier.logical_limit_bytes, Some(2048));
}

#[test]
fn quoted_values_borrow_unescaped_input_and_preserve_escape_rules() {
    for raw in [r#""""#, r#" "hmac-sha256" "#, r#""café""#, r#""inner"quote""#] {
        let trimmed = raw.trim();
        let expected = &trimmed[1..trimmed.len() - 1];
        let (value, stats) =
            crate::test_allocation_counter::count_allocation_stats(|| HmacAuth::parse_quoted_value(raw).unwrap());
        assert!(matches!(value, Cow::Borrowed(_)));
        assert_eq!(value.as_ref(), expected);
        assert_eq!(value.as_ptr(), expected.as_ptr());
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
    for (raw, expected) in [
        (r#""a\"b""#, "a\"b"),
        (r#""a\\b""#, "a\\b"),
        (r#""a\qb""#, "aqb"),
        (r#""\é""#, "é"),
    ] {
        let value = HmacAuth::parse_quoted_value(raw).unwrap();
        assert!(matches!(value, Cow::Owned(_)));
        assert_eq!(value, expected);
    }
    assert_eq!(
        HmacAuth::parse_quoted_value(r#""tail\""#),
        Err("Invalid escape in quoted value")
    );
    for raw in ["", "plain", "\"", "\"open", "close\""] {
        assert_eq!(
            HmacAuth::parse_quoted_value(raw),
            Err("HMAC parameter value must be quoted")
        );
    }
}

#[test]
fn authorization_parser_keeps_owned_results_and_validation_order() {
    let params = {
        let header = String::from(
            r#" HMAC USERNAME="ali\ce", algorithm="hmac-sha256", headers="Date X-Mode", signature="YQ==", extra="a,b\"c" "#,
        );
        HmacAuth::parse_authorization(&header).unwrap()
    };
    assert_eq!(params.username, "alice");
    assert_eq!(params.algorithm, HmacAlgorithm::HmacSha256);
    assert_eq!(params.headers, ["date", "x-mode"]);
    assert_eq!(params.signature, b"a");
    let plain = String::from(r#"hmac username="alice", algorithm="hmac-sha256", headers="date", signature="YQ==""#);
    let owned = HmacAuth::parse_authorization(&plain).unwrap();
    drop(plain);
    assert_eq!(owned.username, "alice");
    for (suffix, expected) in [
        (", extra=unquoted", "HMAC parameter value must be quoted"),
        (", USERNAME=unquoted", "Invalid HMAC authorization format"),
        (r#", extra="tail\""#, "Invalid escape in quoted value"),
        (r#", extra="ok", EXTRA="ok""#, "Invalid HMAC authorization format"),
    ] {
        let header =
            format!(r#"hmac username="alice", algorithm="hmac-sha256", headers="date", signature="YQ=="{suffix}"#);
        assert_eq!(HmacAuth::parse_authorization(&header).unwrap_err(), expected);
    }
    let duplicate_headers = r#"hmac username="alice", algorithm="hmac-sha256", headers="Date date", signature="YQ==""#;
    assert_eq!(
        HmacAuth::parse_authorization(duplicate_headers).unwrap_err(),
        "Invalid HMAC authorization format"
    );
}

#[test]
fn signing_target_borrows_raw_query_without_changing_empty_or_encoded_values() {
    for (query, suffix) in [
        (None, ""),
        (Some(""), ""),
        (Some("q=hello+world&x=a%26b&x=&flag"), "?q=hello+world&x=a%26b&x=&flag"),
    ] {
        for target in ["@request-target", "(request-target)"] {
            let mut session = MockPluginSession::new();
            session.expect_get_path().return_const("/rewritten".to_string());
            session.expect_request_query().times(1).return_const(query);
            session.expect_get_query().never();
            session.expect_get_method().return_const("PoSt".to_string());
            let signing = HmacAuth::build_signing_string(&session, &[target.to_string()]).unwrap();
            assert_eq!(signing, format!("{target}: post /rewritten{suffix}"));
        }
    }
}

#[test]
fn algorithm_selection_does_not_allocate_lowercase_storage() {
    for (value, expected) in [
        ("HmAc-ShA256", Some(HmacAlgorithm::HmacSha256)),
        ("HMAC-SHA384", Some(HmacAlgorithm::HmacSha384)),
        ("hmac-sha512", Some(HmacAlgorithm::HmacSha512)),
        ("hmac-sha256-extra", None),
    ] {
        let (parsed, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            HmacAlgorithm::parse(std::hint::black_box(value))
        });
        assert_eq!(parsed, expected);
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}
