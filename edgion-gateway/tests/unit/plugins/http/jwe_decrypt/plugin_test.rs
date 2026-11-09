use super::*;
use crate::plugins::http::plugin_store::compile_plugins;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, RequestFilterEntry, ResolvedJweCredential};
use edgion_resources::resources::EdgionPlugins;
use mockall::Sequence;
use serde_json::json;
use std::collections::HashMap;

#[tokio::test]
async fn missing_or_non_json_payload_clears_configured_headers_and_propagates_failure() {
    for payload in [None, Some("opaque plaintext"), Some(r#"{"other":true}"#)] {
        for removal_fails in [false, true] {
            let mut config = config_with_key(&test_key());
            config.strict = false;
            config.payload_to_headers = Some(HashMap::from([("sub".into(), "X-Sub".into())]));
            let plugin = JweDecrypt::new(&config, "default".into());
            let token = payload.map(|value| format!("Bearer {}", encrypt_compact_jwe(value, &test_key(), "A256GCM")));
            let mut session = MockPluginSession::new();
            let headers = token
                .as_ref()
                .map(|value| vec![("authorization", value.as_str())])
                .unwrap_or_default();
            setup_header_mocks(&mut session, &headers);
            session
                .expect_set_request_header_value()
                .times(usize::from(payload.is_some()))
                .returning(|_, _| Ok(()));
            session
                .expect_remove_request_header()
                .withf(|name| name == "x-sub")
                .times(1)
                .returning(move |_| {
                    if removal_fails {
                        Err(std::io::Error::other("private failure").into())
                    } else {
                        Ok(())
                    }
                });
            // The no-token bypass also owns `forwardHeader`; it is cleared after
            // the projections (never reached when a projection clear failed).
            session
                .expect_remove_request_header()
                .withf(|name| name == "x-decrypted-auth")
                .times(usize::from(payload.is_none() && !removal_fails))
                .returning(|_| Ok(()));
            if removal_fails {
                // Same response a bad token gets on that path: decrypt failure with a
                // token, "token required" on the non-strict no-token bypass.
                let body: &'static [u8] = if payload.is_some() {
                    b"401 JWE decryption failed"
                } else {
                    b"401 JWE token required"
                };
                expect_header_deny(&mut session, body);
            }
            let mut log = PluginLog::new("JweDecrypt");
            let result = plugin.run_request(&mut session, &mut log).await;
            if removal_fails {
                assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
                assert!(log.contains(CLAIM_HEADER_MUTATION_FAILED), "got: {log:?}");
                assert!(!log.contains("OK"), "got: {log:?}");
                assert!(!log.contains("bypass"), "got: {log:?}");
            } else {
                assert_eq!(result, PluginRunningResult::GoodNext);
            }
            assert!(!log.contains("private"));
        }
    }
}

/// Expect one recorded `AuthHeaderMutationRejected`, then the ordinary 401 response.
fn expect_header_deny(session: &mut MockPluginSession, body: &'static [u8]) {
    let mut sequence = Sequence::new();
    session
        .expect_record_error()
        .withf(|code| *code == edgion_resources::EdgionErrorCode::AuthHeaderMutationRejected)
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(());
    expect_auth_response(session, &mut sequence, 401, body);
}

#[tokio::test(start_paused = true)]
async fn payload_header_mutation_deny_applies_the_failure_delay() {
    use tokio::time::{advance, Duration, Instant};
    let mut config = config_with_key(&test_key());
    config.auth_failure_delay_ms = 100;
    config.payload_to_headers = Some(HashMap::from([("sub".into(), "X-Sub".into())]));
    let plugin = JweDecrypt::new(&config, "default".into());
    let token = format!(
        "Bearer {}",
        encrypt_compact_jwe(r#"{"sub":"trusted"}"#, &test_key(), "A256GCM")
    );
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", token.as_str())]);
    session
        .expect_set_request_header_value()
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_request_header()
        .times(1)
        .returning(|_| Err(std::io::Error::other("injected").into()));
    expect_header_deny(&mut session, b"401 JWE decryption failed");

    let start = Instant::now();
    let task = tokio::spawn(async move {
        let mut log = PluginLog::new("JweDecrypt");
        let result = plugin.run_request(&mut session, &mut log).await;
        (result, log)
    });
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    advance(Duration::from_millis(99)).await;
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    advance(Duration::from_millis(1)).await;
    let (result, log) = task.await.unwrap();
    assert_eq!(Instant::now().duration_since(start), Duration::from_millis(100));
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains(CLAIM_HEADER_MUTATION_FAILED));
}

#[tokio::test]
async fn bypass_removes_forged_forward_header() {
    let mut config = config_with_key(&test_key());
    config.strict = false;
    let plugin = JweDecrypt::new(&config, "default".into());
    let mut session = MockPluginSession::new();
    // A client-forged decrypted payload must not reach the upstream.
    setup_header_mocks(&mut session, &[("x-decrypted-auth", "forged")]);
    session.expect_set_request_header_value().never();
    session
        .expect_remove_request_header()
        .withf(|name| name == "x-decrypted-auth")
        .times(1)
        .returning(|_| Ok(()));
    session.expect_record_error().never();
    let mut log = PluginLog::new("JweDecrypt");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(log.render_for_test(), "bypass");
}

#[tokio::test]
async fn bypass_forward_header_removal_failure_denies_token_required() {
    let mut config = config_with_key(&test_key());
    config.strict = false;
    let plugin = JweDecrypt::new(&config, "default".into());
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("x-decrypted-auth", "forged")]);
    session
        .expect_remove_request_header()
        .withf(|name| name == "x-decrypted-auth")
        .times(1)
        .returning(|_| Err(std::io::Error::other("injected").into()));
    expect_header_deny(&mut session, b"401 JWE token required");
    let mut log = PluginLog::new("JweDecrypt");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains(CLAIM_HEADER_MUTATION_FAILED), "got: {log:?}");
    assert!(!log.contains("bypass"), "got: {log:?}");
}

#[tokio::test]
async fn hidden_credential_removal_failure_denies_with_decrypt_failure() {
    let mut config = config_with_key(&test_key());
    config.hide_credentials = true;
    let plugin = JweDecrypt::new(&config, "default".into());
    let token = format!(
        "Bearer {}",
        encrypt_compact_jwe(r#"{"sub":"alice"}"#, &test_key(), "A256GCM")
    );
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", token.as_str())]);
    session
        .expect_set_request_header_value()
        .withf(|name, _| name == "x-decrypted-auth")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|name| name == "authorization")
        .times(1)
        .returning(|_| Err(std::io::Error::other("injected").into()));
    expect_header_deny(&mut session, b"401 JWE decryption failed");
    let mut log = PluginLog::new("JweDecrypt");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains(CREDENTIAL_REMOVAL_FAILED), "got: {log:?}");
    assert!(!log.contains("OK"), "got: {log:?}");
}

fn test_key() -> Vec<u8> {
    b"0123456789abcdef0123456789abcdef".to_vec()
}

fn config_with_key(key: &[u8]) -> JweDecryptConfig {
    JweDecryptConfig {
        secret_ref: Some(edgion_resources::resources::gateway::SecretObjectReference {
            group: None,
            kind: None,
            name: "jwe-secret".to_string(),
            namespace: None,
        }),
        strip_prefix: Some("Bearer ".to_string()),
        forward_header: "x-decrypted-auth".to_string(),
        resolved_credential: Some(ResolvedJweCredential {
            secret: Some(STANDARD.encode(key)),
        }),
        ..Default::default()
    }
}

fn build_plugin(config_fn: Option<fn(&mut JweDecryptConfig)>) -> JweDecrypt {
    let mut config = config_with_key(&test_key());
    if let Some(f) = config_fn {
        f(&mut config);
    }
    JweDecrypt::new(&config, "default".to_string())
}

fn encrypt_compact_jwe_with_protected(payload: &str, key: &[u8], protected: &str, iv_byte: u8) -> String {
    let protected_segment = URL_SAFE_NO_PAD.encode(protected.as_bytes());
    let iv = [iv_byte; 12];
    let mut ciphertext = payload.as_bytes().to_vec();
    let cipher = Aes256Gcm::new_from_slice(key).unwrap();
    let nonce = Nonce::from_slice(&iv);
    let tag = cipher
        .encrypt_in_place_detached(nonce, protected_segment.as_bytes(), &mut ciphertext)
        .unwrap();

    format!(
        "{}..{}.{}.{}",
        protected_segment,
        URL_SAFE_NO_PAD.encode(iv),
        URL_SAFE_NO_PAD.encode(ciphertext),
        URL_SAFE_NO_PAD.encode(tag)
    )
}

fn encrypt_compact_jwe(payload: &str, key: &[u8], enc: &str) -> String {
    let protected = json!({
        "alg": "dir",
        "enc": enc
    })
    .to_string();
    encrypt_compact_jwe_with_protected(payload, key, &protected, 1)
}

fn decrypt_with_prepared(plugin: &JweDecrypt, token: &str) -> Result<String, JweFailure> {
    match &plugin.decryptor {
        PreparedDecryptor::Ready(cipher) => plugin.decrypt_compact_jwe(token, cipher),
        PreparedDecryptor::InvalidLength => Err(JweFailure::KeyLenErr),
        PreparedDecryptor::Missing | PreparedDecryptor::DecodeFailed => Err(JweFailure::NoKey),
    }
}

fn expect_auth_response(session: &mut MockPluginSession, sequence: &mut Sequence, status: u16, body: &'static [u8]) {
    session
        .expect_write_response_header()
        .withf(move |response, end_of_stream| {
            !end_of_stream
                && response.status.as_u16() == status
                && response
                    .headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    == Some("text/plain")
                && response
                    .headers
                    .get("www-authenticate")
                    .and_then(|value| value.to_str().ok())
                    == Some("Bearer realm=\"edgion\"")
        })
        .times(1)
        .in_sequence(sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_write_response_body()
        .withf(move |response_body, end_of_stream| {
            *end_of_stream && response_body.as_deref() == Some(Bytes::from_static(body).as_ref())
        })
        .times(1)
        .in_sequence(sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(sequence)
        .returning(|| {});
}

#[tokio::test]
async fn test_missing_token_strict_true_reject() {
    let plugin = build_plugin(None);
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("JweDecrypt");

    setup_header_mocks(&mut session, &[]);
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("no token"));
}

#[tokio::test]
async fn test_token_too_large_reject() {
    let plugin = build_plugin(None);
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("JweDecrypt");

    let large = format!("Bearer {}", "a".repeat(9000));
    setup_header_mocks(&mut session, &[("authorization", large.as_str())]);
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("token too large"));
}

#[tokio::test]
async fn test_bad_algorithm_reject() {
    let plugin = build_plugin(None);
    let token = encrypt_compact_jwe(r#"{"uid":"100"}"#, &test_key(), "A128GCM");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("JweDecrypt");

    let header_value = format!("Bearer {}", token);
    setup_header_mocks(&mut session, &[("authorization", header_value.as_str())]);
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("bad alg"));
}

#[test]
fn direct_constructor_defensively_classifies_key_material_and_drops_retained_credential() {
    let ready = JweDecrypt::new(&config_with_key(&test_key()), "default".to_string());
    assert!(matches!(ready.decryptor, PreparedDecryptor::Ready(_)));
    assert!(ready.config.resolved_credential.is_none());

    let mut missing_config = config_with_key(&test_key());
    missing_config.resolved_credential = Some(ResolvedJweCredential { secret: None });
    let missing = JweDecrypt::new(&missing_config, "default".to_string());
    assert!(matches!(missing.decryptor, PreparedDecryptor::Missing));
    assert!(missing.config.resolved_credential.is_none());

    let mut decode_config = config_with_key(&test_key());
    decode_config.resolved_credential = Some(ResolvedJweCredential {
        secret: Some("not-base64!".to_string()),
    });
    let decode = JweDecrypt::new(&decode_config, "default".to_string());
    assert!(matches!(decode.decryptor, PreparedDecryptor::DecodeFailed));
    assert!(decode.config.resolved_credential.is_none());

    let mut inner_decode_config = config_with_key(&test_key());
    inner_decode_config.base64_secret = true;
    inner_decode_config.resolved_credential = Some(ResolvedJweCredential {
        secret: Some(STANDARD.encode("not-base64!")),
    });
    let inner_decode = JweDecrypt::new(&inner_decode_config, "default".to_string());
    assert!(matches!(inner_decode.decryptor, PreparedDecryptor::DecodeFailed));
    assert!(inner_decode.config.resolved_credential.is_none());

    let invalid_length = JweDecrypt::new(&config_with_key(b"too-short"), "default".to_string());
    assert!(matches!(invalid_length.decryptor, PreparedDecryptor::InvalidLength));
    assert!(invalid_length.config.resolved_credential.is_none());
}

#[test]
fn prepared_cipher_is_send_sync_reusable_and_generation_isolated() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Aes256Gcm>();
    assert_send_sync::<JweDecrypt>();

    let key_one = [0x11; 32];
    let key_two = [0x22; 32];
    let plugin_one = JweDecrypt::new(&config_with_key(&key_one), "default".to_string());
    let plugin_two = JweDecrypt::new(&config_with_key(&key_two), "default".to_string());
    let token_one = encrypt_compact_jwe_with_protected(
        r#"{"generation":"one"}"#,
        &key_one,
        r#"{"alg":"dir","enc":"A256GCM"}"#,
        2,
    );
    let token_two = encrypt_compact_jwe_with_protected(
        r#"{"generation":"two"}"#,
        &key_two,
        r#"{"alg":"dir","enc":"A256GCM"}"#,
        3,
    );

    assert_eq!(
        decrypt_with_prepared(&plugin_one, &token_one).unwrap(),
        r#"{"generation":"one"}"#
    );
    assert_eq!(
        decrypt_with_prepared(&plugin_one, &token_one).unwrap(),
        r#"{"generation":"one"}"#
    );
    assert_eq!(
        decrypt_with_prepared(&plugin_one, &token_two),
        Err(JweFailure::DecryptFail)
    );
    assert_eq!(
        decrypt_with_prepared(&plugin_two, &token_two).unwrap(),
        r#"{"generation":"two"}"#
    );
    assert_eq!(
        decrypt_with_prepared(&plugin_two, &token_one),
        Err(JweFailure::DecryptFail)
    );
    assert!(plugin_one.config.resolved_credential.is_none());
    assert!(plugin_two.config.resolved_credential.is_none());
}

#[tokio::test]
async fn protected_zip_and_crit_are_rejected_on_presence() {
    let cases = [
        r#"{"alg":"dir","enc":"A256GCM","crit":null}"#,
        r#"{"alg":"dir","enc":"A256GCM","crit":[]}"#,
        r#"{"alg":"dir","enc":"A256GCM","crit":["exp"],"exp":true}"#,
        r#"{"alg":"dir","enc":"A256GCM","zip":null}"#,
        r#"{"alg":"dir","enc":"A256GCM","zip":"DEF"}"#,
    ];

    for (index, protected) in cases.into_iter().enumerate() {
        let plugin = build_plugin(None);
        let token = encrypt_compact_jwe_with_protected(r#"{"sub":"alice"}"#, &test_key(), protected, index as u8 + 10);
        let header_value = format!("Bearer {token}");
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("authorization", header_value.as_str())]);
        let mut sequence = Sequence::new();
        expect_auth_response(&mut session, &mut sequence, 400, b"400 Invalid JWE format");

        let mut log = PluginLog::new("JweDecrypt");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert_eq!(log.render_for_test(), "invalid format");
    }
}

#[test]
fn duplicate_protected_header_members_are_rejected() {
    let plugin = build_plugin(None);
    let cases = [
        r#"{"alg":"dir","alg":"dir","enc":"A256GCM"}"#,
        r#"{"alg":"dir","enc":"A256GCM","enc":"A256GCM"}"#,
        r#"{"alg":"dir","enc":"A256GCM","x-private":1,"x-private":2}"#,
        r#"{"alg":"dir","\u0061lg":"dir","enc":"A256GCM"}"#,
        r#"{"\u0061lg":"dir","alg":"dir","enc":"A256GCM"}"#,
        r#"{"alg":"dir","enc":"A256GCM","x-private":1,"x\u002dprivate":2}"#,
        r#"{"alg":"dir","enc":"A256GCM","x\u002dprivate":1,"x-private":2}"#,
    ];

    for (index, protected) in cases.into_iter().enumerate() {
        let token = encrypt_compact_jwe_with_protected(r#"{"sub":"alice"}"#, &test_key(), protected, index as u8 + 30);
        assert_eq!(
            decrypt_with_prepared(&plugin, &token),
            Err(JweFailure::InvalidFormat),
            "duplicate protected header member was accepted: {protected}"
        );
    }
}

#[test]
fn escaped_protected_header_names_keep_raw_aad_and_decoded_semantics() {
    let plugin = build_plugin(None);
    let plaintext = r#"{"sub":"alice"}"#;
    for (index, protected) in [
        r#"{"\u0061lg":"dir","\u0065nc":"A256GCM","x\u002dprivate":1}"#,
        r#"{"alg":"dir","enc":"A256GCM","x\u002dprivate":1,"y-private":2}"#,
        r#"{"alg":"dir","enc":"A256GCM","x\u0022name":"ignored"}"#,
    ]
    .into_iter()
    .enumerate()
    {
        let token = encrypt_compact_jwe_with_protected(plaintext, &test_key(), protected, index as u8 + 50);
        assert_eq!(decrypt_with_prepared(&plugin, &token).unwrap(), plaintext);
    }
}

#[test]
fn non_object_protected_headers_are_invalid_format() {
    let plugin = build_plugin(None);
    let cases = [r#"[]"#, r#""header""#, "42", "true", "null"];

    for (index, protected) in cases.into_iter().enumerate() {
        let token = encrypt_compact_jwe_with_protected(r#"{"sub":"alice"}"#, &test_key(), protected, index as u8 + 40);
        assert_eq!(
            decrypt_with_prepared(&plugin, &token),
            Err(JweFailure::InvalidFormat),
            "non-object protected header was not invalid format: {protected}"
        );
    }
}

#[tokio::test]
async fn unknown_noncritical_header_uses_raw_aad_and_preserves_success_order() {
    let mut config = config_with_key(&test_key());
    config.hide_credentials = true;
    config.store_payload_in_ctx = true;
    config.payload_to_headers = Some(HashMap::from([("sub".to_string(), "X-Sub".to_string())]));
    let plugin = JweDecrypt::new(&config, "default".to_string());
    let plaintext = r#"{"sub":"alice"}"#;
    let protected = r#"{ "enc":"A256GCM", "x-private":"accepted", "alg":"dir" }"#;
    let token = encrypt_compact_jwe_with_protected(plaintext, &test_key(), protected, 20);
    let header_value = format!("Bearer {token}");
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", header_value.as_str())]);

    session
        .expect_remove_request_header()
        .withf(|name| name == "x-sub")
        .times(1)
        .returning(|_| Ok(()));
    let mut sequence = Sequence::new();
    session
        .expect_set_request_header_value()
        .withf(move |name, value| name == "x-decrypted-auth" && value == plaintext && value.is_sensitive())
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "x-sub" && value == "alice" && value.is_sensitive())
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_set_ctx_var()
        .withf(move |name, value| name == "jwe_payload" && value == plaintext)
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|name| name == "authorization")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_| Ok(()));

    let mut log = PluginLog::new("JweDecrypt");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "OK");
}

#[test]
fn construction_compiles_payload_projection_in_canonical_order() {
    let mut config = config_with_key(&test_key());
    config.payload_to_headers = Some(HashMap::from([
        ("tenant".to_string(), "X-Tenant".to_string()),
        ("subject".to_string(), "X-Subject".to_string()),
    ]));

    let plugin = JweDecrypt::new(&config, "default".to_string());
    let entries = plugin.claim_header_projection.entries();
    assert_eq!(entries[0].claim_path(), "subject");
    assert_eq!(entries[0].header_name(), "x-subject");
    assert_eq!(entries[1].claim_path(), "tenant");
    assert_eq!(entries[1].header_name(), "x-tenant");
}

#[tokio::test]
async fn payload_projection_enforces_its_own_exact_and_plus_one_caps() {
    let plaintext = r#"{"sub":"alice"}"#;
    let token = encrypt_compact_jwe(plaintext, &test_key(), "A256GCM");

    for (max_value, max_total, writes_projection) in [(5, 10, true), (4, 10, false), (5, 9, false)] {
        let mut config = config_with_key(&test_key());
        config.payload_to_headers = Some(HashMap::from([("sub".to_string(), "X-Sub".to_string())]));
        config.max_header_value_bytes = max_value;
        config.max_total_header_bytes = max_total;
        let plugin = JweDecrypt::new(&config, "default".to_string());
        let header_value = format!("Bearer {token}");
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("authorization", header_value.as_str())]);
        session
            .expect_remove_request_header()
            .withf(|name| name == "x-sub")
            .times(1)
            .returning(|_| Ok(()));
        let mut sequence = Sequence::new();

        session
            .expect_set_request_header_value()
            .withf(move |name, value| name == "x-decrypted-auth" && value == plaintext && value.is_sensitive())
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        if writes_projection {
            session
                .expect_set_request_header_value()
                .withf(|name, value| name == "x-sub" && value == "alice" && value.is_sensitive())
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_, _| Ok(()));
        }

        let mut log = PluginLog::new("JweDecrypt");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }
}

#[tokio::test]
async fn direct_constructor_unavailable_key_precedes_parsing_with_one_generic_response_and_log() {
    let cases = [
        Some(ResolvedJweCredential { secret: None }),
        Some(ResolvedJweCredential {
            secret: Some("not-base64!".to_string()),
        }),
    ];

    for resolved_credential in cases {
        let mut config = config_with_key(&test_key());
        config.resolved_credential = resolved_credential;
        let plugin = JweDecrypt::new(&config, "default".to_string());
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("authorization", "Bearer malformed")]);
        let mut sequence = Sequence::new();
        expect_auth_response(&mut session, &mut sequence, 401, b"401 JWE decryption failed");

        let mut log = PluginLog::new("JweDecrypt");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert_eq!(log.render_for_test(), "err: key load fail");
    }
}

#[tokio::test]
async fn direct_constructor_invalid_key_length_has_one_generic_response_and_distinct_log() {
    let plugin = JweDecrypt::new(&config_with_key(b"too-short"), "default".to_string());
    let token = encrypt_compact_jwe(r#"{"sub":"alice"}"#, &test_key(), "A256GCM");
    let header_value = format!("Bearer {token}");
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", header_value.as_str())]);
    let mut sequence = Sequence::new();
    expect_auth_response(&mut session, &mut sequence, 401, b"401 JWE decryption failed");

    let mut log = PluginLog::new("JweDecrypt");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert_eq!(log.render_for_test(), "err: key len err");
}

#[tokio::test]
async fn direct_constructor_rejects_invalid_key_length_before_parsing() {
    let plugin = JweDecrypt::new(&config_with_key(b"too-short"), "default".to_string());
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", "Bearer malformed")]);
    let mut sequence = Sequence::new();
    expect_auth_response(&mut session, &mut sequence, 401, b"401 JWE decryption failed");

    let mut log = PluginLog::new("JweDecrypt");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert_eq!(log.render_for_test(), "err: key len err");
}

#[tokio::test]
async fn compile_plugins_constructs_fail_closed_jwe_for_invalid_resolved_credentials() {
    async fn assert_constructed(config: JweDecryptConfig, expected_log: &str) {
        let mut resource = EdgionPlugins::new("invalid-jwe", Default::default());
        resource.metadata.namespace = Some("default".to_string());
        resource.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::JweDecrypt(config))]);

        let compiled = compile_plugins(&resource);
        assert!(compiled.is_activatable);
        assert!(!compiled.retain_previous_on_invalid_update);
        assert!(compiled.preparse_errors.is_empty());
        assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);

        let filter = compiled
            .plugin_runtime
            .request_plugins_iter()
            .next()
            .expect("constructed JWE filter");
        assert_eq!(filter.name(), "JweDecrypt");

        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("authorization", "Bearer malformed")]);
        let mut sequence = Sequence::new();
        expect_auth_response(&mut session, &mut sequence, 401, b"401 JWE decryption failed");
        let mut log = PluginLog::new(filter.name());
        let result = filter.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert_eq!(log.render_for_test(), expected_log);
    }

    let mut invalid_encoding = config_with_key(&test_key());
    invalid_encoding.resolved_credential = Some(ResolvedJweCredential {
        secret: Some("not-base64!".to_string()),
    });
    assert_constructed(invalid_encoding, "err: key load fail").await;

    let invalid_length = config_with_key(b"too-short");
    assert_constructed(invalid_length, "err: key len err").await;
}

#[test]
fn token_extraction_borrows_the_selected_header_without_allocation() {
    for (prefix, raw, expected) in [
        (Some("Bearer "), "  Bearer token.parts  ", Some("token.parts")),
        (Some("Bearer "), "Bearer   token.parts \t", Some("token.parts")),
        (Some("Bearer "), "bearer token.parts", Some("bearer token.parts")),
        (Some("Bearer "), "Basic token.parts", Some("Basic token.parts")),
        (Some("Bearer "), "Bearer ", Some("Bearer")),
        (Some("Bearer"), "Bearer \t", None),
        (Some(""), " token.parts ", Some("token.parts")),
        (None, " Bearer token.parts ", Some("Bearer token.parts")),
        (None, " \t ", None),
        (None, "", None),
    ] {
        let mut config = config_with_key(&test_key());
        config.strip_prefix = prefix.map(str::to_owned);
        let plugin = JweDecrypt::new(&config, "default".to_owned());
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("authorization", raw), ("authorization", "ignored")]);
        let selected = session.header_str("authorization").unwrap();
        let (token, stats) = crate::test_allocation_counter::count_allocation_stats(|| plugin.extract_token(&session));
        assert_eq!(token, expected);
        assert_eq!(stats.calls, 0, "{stats:?}");
        if let Some(token) = token {
            let offset = selected.find(token).unwrap();
            assert_eq!(token.as_ptr(), selected[offset..].as_ptr());
        }
    }

    let plugin = build_plugin(None);
    for value in [None, Some(http::HeaderValue::from_bytes(b"\xff").unwrap())] {
        let mut headers = http::HeaderMap::new();
        if let Some(value) = value {
            headers.append("authorization", value);
            headers.append("authorization", http::HeaderValue::from_static("Bearer ignored"));
        }
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);
        assert_eq!(plugin.extract_token(&session), None);
    }
}

#[test]
fn compact_structure_rejection_needs_no_segment_vector() {
    let plugin = build_plugin(None);
    let mut inputs: Vec<String> = [
        "",
        "a",
        "a.",
        "a..",
        "a..c",
        "a..c.d",
        "a..c.d.e.",
        "a..c.d.e.f",
        "..c.d.e",
        "a.b.c.d.e",
        "a...d.e",
        "a..c..e",
        "a..c.d.",
        "....",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    inputs.push(".".repeat(8192));
    for input in inputs {
        let (result, stats) =
            crate::test_allocation_counter::count_allocation_stats(|| plugin.parse_compact_jwe(&input));
        assert!(matches!(result, Err(JweFailure::InvalidFormat)), "input={input:?}");
        assert_eq!(stats.calls, 0, "input={input:?}, {stats:?}");
    }
}

#[test]
fn compact_segment_views_preserve_original_encoded_slices() {
    let plugin = build_plugin(None);
    let token = encrypt_compact_jwe_with_protected("payload", &test_key(), r#"{ "enc":"A256GCM", "alg":"dir" }"#, 4);
    let original_segments: Vec<_> = token.split('.').collect();
    let parsed = plugin.parse_compact_jwe(&token).unwrap();
    for (actual, expected) in [
        (parsed.protected_segment, original_segments[0]),
        (parsed.iv_segment, original_segments[2]),
        (parsed.ciphertext_segment, original_segments[3]),
        (parsed.tag_segment, original_segments[4]),
    ] {
        assert_eq!(actual, expected);
        assert_eq!(actual.as_ptr(), expected.as_ptr());
    }
    assert_eq!(decrypt_with_prepared(&plugin, &token).unwrap(), "payload");
}

fn original_decrypt_compact_jwe(plugin: &JweDecrypt, token: &str, cipher: &Aes256Gcm) -> Result<String, JweFailure> {
    let parsed = plugin.parse_compact_jwe(token)?;

    let iv = URL_SAFE_NO_PAD
        .decode(parsed.iv_segment)
        .map_err(|_| JweFailure::InvalidFormat)?;
    let mut ciphertext = URL_SAFE_NO_PAD
        .decode(parsed.ciphertext_segment)
        .map_err(|_| JweFailure::InvalidFormat)?;
    let tag_bytes = URL_SAFE_NO_PAD
        .decode(parsed.tag_segment)
        .map_err(|_| JweFailure::InvalidFormat)?;

    // RFC 7518 A256GCM uses 96-bit IV and 128-bit tag.
    if iv.len() != 12 || tag_bytes.len() != 16 {
        return Err(JweFailure::InvalidFormat);
    }

    let nonce = Nonce::from_slice(&iv);
    let tag = Tag::from_slice(&tag_bytes);

    cipher
        .decrypt_in_place_detached(nonce, parsed.protected_segment.as_bytes(), &mut ciphertext, tag)
        .map_err(|_| JweFailure::DecryptFail)?;

    String::from_utf8(ciphertext).map_err(|_| JweFailure::InvalidFormat)
}

#[test]
fn fixed_iv_and_tag_decode_preserve_original_results() {
    let plugin = build_plugin(None);
    let cipher = Aes256Gcm::new_from_slice(&test_key()).unwrap();
    let token = encrypt_compact_jwe("payload", &test_key(), "A256GCM");
    let segments: Vec<_> = token.split('.').collect();
    assert_eq!(segments[4].len(), 22);
    let mut inputs = vec![token.clone()];
    for field in [2, 4] {
        let mut replacements = Vec::new();
        for len in 0..=33 {
            replacements.push(URL_SAFE_NO_PAD.encode(vec![7; len]));
        }
        replacements.extend([
            "=".to_owned(),
            "a".to_owned(),
            "%%%".to_owned(),
            format!("{}=", segments[field]),
        ]);
        for byte in 0u8..=127 {
            let mut replacement = segments[field].as_bytes().to_vec();
            *replacement.last_mut().unwrap() = byte;
            replacements.push(String::from_utf8(replacement).unwrap());
        }
        for replacement in replacements {
            let mut changed = segments.clone();
            changed[field] = &replacement;
            inputs.push(changed.join("."));
        }
    }
    for (iv, ciphertext, tag) in [
        (URL_SAFE_NO_PAD.encode([0; 13]), "%".to_owned(), "%".to_owned()),
        (URL_SAFE_NO_PAD.encode([0; 11]), "%".to_owned(), "%".to_owned()),
        (segments[2].to_owned(), "%".to_owned(), URL_SAFE_NO_PAD.encode([0; 17])),
        (
            segments[2].to_owned(),
            URL_SAFE_NO_PAD.encode([0; 7]),
            segments[4].to_owned(),
        ),
    ] {
        inputs.push(format!("{}..{iv}.{ciphertext}.{tag}", segments[0]));
    }
    for input in inputs {
        assert_eq!(
            plugin.decrypt_compact_jwe(&input, &cipher),
            original_decrypt_compact_jwe(&plugin, &input, &cipher),
            "input={input:?}"
        );
    }
}

#[test]
fn fixed_iv_and_tag_remove_two_decode_allocations() {
    let plugin = build_plugin(None);
    let cipher = Aes256Gcm::new_from_slice(&test_key()).unwrap();
    for payload in ["payload", r#"{"sub":"alice","role":"reader"}"#] {
        let token = encrypt_compact_jwe(payload, &test_key(), "A256GCM");
        let (old, old_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            original_decrypt_compact_jwe(&plugin, &token, &cipher)
        });
        let (new, new_stats) =
            crate::test_allocation_counter::count_allocation_stats(|| plugin.decrypt_compact_jwe(&token, &cipher));
        assert_eq!(new.as_deref(), Ok(payload));
        assert_eq!(new, old);
        assert_eq!(
            new_stats.calls + 2,
            old_stats.calls,
            "old={old_stats:?}, new={new_stats:?}"
        );
        assert!(
            new_stats.requested_bytes < old_stats.requested_bytes,
            "old={old_stats:?}, new={new_stats:?}"
        );
    }
}
