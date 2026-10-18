use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use bytes::Bytes;
use edgion_resources::sensitive::SENSITIVE_MODE;
use mockall::Sequence;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn setup_key_headers(session: &mut MockPluginSession, key: Option<&str>) {
    let mut headers = http::HeaderMap::new();
    if let Some(key) = key {
        headers.insert("x-api-key", http::HeaderValue::from_str(key).unwrap());
    }
    session.expect_request_header_map().return_const(headers);
    session.expect_key_get().times(0);
}

fn create_test_config() -> KeyAuthConfig {
    KeyAuthConfig {
        key_sources: vec![
            KeyGet::Header {
                name: "X-API-Key".to_string(),
            },
            KeyGet::Query {
                name: "api_key".to_string(),
            },
        ],
        hide_credentials: false,
        auth_failure_delay_ms: 0,
        anonymous: None,
        realm: "Test API".to_string(),
        key_field: "key".to_string(),
        secret_groups: None,
        upstream_header_fields: vec!["X-Consumer-Username".to_string(), "X-Customer-ID".to_string()],
        resolved_keys: None,
    }
}

fn create_key_auth_with_keys() -> KeyAuth {
    let mut config = create_test_config();
    let mut keys = HashMap::new();
    let group: String = "partner-a".to_string();

    // Key 1: jack
    let mut jack_headers = HashMap::new();
    jack_headers.insert("X-Consumer-Username".to_string(), "jack".to_string());
    jack_headers.insert("X-Customer-ID".to_string(), "cust-001".to_string());
    keys.insert(
        "jack-key-12345".to_string(),
        KeyMetadata {
            headers: jack_headers,
            group: group.clone(),
        },
    );

    // Key 2: alice
    let mut alice_headers = HashMap::new();
    alice_headers.insert("X-Consumer-Username".to_string(), "alice".to_string());
    alice_headers.insert("X-Customer-ID".to_string(), "cust-002".to_string());
    keys.insert(
        "alice-key-67890".to_string(),
        KeyMetadata {
            headers: alice_headers,
            group: group.clone(),
        },
    );

    config.resolved_keys = Some(keys);
    KeyAuth::from_config(&config)
}

fn expect_auth_response(mock: &mut MockPluginSession, expected_body: &'static [u8]) {
    let mut sequence = Sequence::new();
    mock.expect_write_response_header()
        .withf(|response, end_of_stream| {
            !end_of_stream
                && response.status.as_u16() == 401
                && response
                    .headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    == Some("text/plain")
                && response
                    .headers
                    .get("www-authenticate")
                    .and_then(|value| value.to_str().ok())
                    == Some("ApiKey realm=\"Test API\"")
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    mock.expect_write_response_body()
        .withf(move |body, end_of_stream| {
            *end_of_stream && body.as_deref() == Some(Bytes::from_static(expected_body).as_ref())
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    mock.expect_shutdown()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|| {});
}

#[test]
fn name_is_static() {
    let auth = KeyAuth::from_config(&create_test_config());

    assert_eq!(auth.name(), PLUGIN_NAME);
    assert_eq!(auth.name().as_ptr(), PLUGIN_NAME.as_ptr());
}

#[test]
fn constructor_leaves_source_config_unchanged_and_keeps_only_runtime_state() {
    let mut config = create_test_config();
    config.hide_credentials = true;
    config.auth_failure_delay_ms = 23;
    config.anonymous = Some("guest".to_string());
    config.key_field = "controller-key-field".to_string();
    config.resolved_keys = Some(HashMap::from([(
        "raw-runtime-key".to_string(),
        metadata_with_group("partner-a", Some("alice")),
    )]));
    let before = serde_json::to_value(&config).unwrap();

    let auth = KeyAuth::from_config(&config);

    assert_eq!(serde_json::to_value(&config).unwrap(), before);
    assert!(config
        .resolved_keys
        .as_ref()
        .is_some_and(|keys| keys.contains_key("raw-runtime-key")));
    assert_eq!(auth.key_sources, config.key_sources);
    assert!(auth.hide_credentials);
    assert_eq!(auth.auth_failure_delay_ms, 23);
    assert_eq!(auth.anonymous.as_deref(), Some("guest"));
    assert_eq!(auth.realm, "Test API");
    assert!(auth.forward_anonymous_username);
    assert_eq!(auth.keys.len(), 1);
    assert!(auth.keys.contains_key(&sha256_of("raw-runtime-key")));
    assert!(auth.validate_key("raw-runtime-key").is_some());
}

#[test]
fn generations_keep_independent_hashed_key_maps() {
    let mut first = create_test_config();
    first.resolved_keys = Some(HashMap::from([(
        "generation-k1".to_string(),
        metadata_with_group("one", None),
    )]));
    let mut second = create_test_config();
    second.resolved_keys = Some(HashMap::from([(
        "generation-k2".to_string(),
        metadata_with_group("two", None),
    )]));

    let k1 = KeyAuth::from_config(&first);
    let k2 = KeyAuth::from_config(&second);

    assert!(k1.validate_key("generation-k1").is_some());
    assert!(k1.validate_key("generation-k2").is_none());
    assert!(k2.validate_key("generation-k1").is_none());
    assert!(k2.validate_key("generation-k2").is_some());
}

#[test]
fn test_extract_key_none() {
    let auth = create_key_auth_with_keys();
    let mut mock_session = MockPluginSession::new();

    setup_key_headers(&mut mock_session, None);
    mock_session.expect_get_query_param_ref().returning(|_| None);

    let result = auth.extract_key(&mock_session);
    assert!(result.is_none());
}

#[test]
fn extraction_returns_the_configured_source_by_reference() {
    let auth = create_key_auth_with_keys();
    let query_reads = Arc::new(AtomicUsize::new(0));
    let query_reads_for_mock = Arc::clone(&query_reads);
    let mut session = MockPluginSession::new();
    setup_key_headers(&mut session, Some("header-present"));
    session.expect_get_query_param_ref().returning(move |_| {
        query_reads_for_mock.fetch_add(1, Ordering::SeqCst);
        Some("query-value")
    });

    let (key, source) = auth.extract_key(&session).expect("header key should win");
    assert_eq!(key, "header-present");
    assert!(std::ptr::eq(source, &auth.key_sources[0]));
    assert_eq!(query_reads.load(Ordering::SeqCst), 0);
}

#[test]
fn test_validate_key_valid() {
    let auth = create_key_auth_with_keys();

    let metadata = auth.validate_key("jack-key-12345");
    assert!(metadata.is_some());

    let meta = metadata.unwrap();
    assert_eq!(meta.headers.get("X-Consumer-Username"), Some(&"jack".to_string()));
    assert_eq!(meta.headers.get("X-Customer-ID"), Some(&"cust-001".to_string()));
}

#[test]
fn test_validate_key_invalid() {
    let auth = create_key_auth_with_keys();

    let metadata = auth.validate_key("invalid-key");
    assert!(metadata.is_none());
}

use crate::plugins::runtime::PluginLog;
use serde_json::Value;

fn metadata_with_group(group: &str, username: Option<&str>) -> KeyMetadata {
    let mut headers = HashMap::new();
    if let Some(u) = username {
        headers.insert("X-Consumer-Username".to_string(), u.to_string());
    }
    KeyMetadata {
        headers,
        group: group.to_string(),
    }
}

fn render_log(log: &PluginLog) -> Vec<String> {
    let v: Value = serde_json::to_value(log).unwrap();
    v.get("log")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

#[test]
fn hit_log_contains_group_and_user() {
    let mut log = PluginLog::new("KeyAuth");
    let meta = metadata_with_group("partner-a", Some("jack"));
    if let Some(user) = meta.headers.get("X-Consumer-Username") {
        log.push_fmt("OK group={} user {}", la![meta.group.as_ref(), sensitive(user)]);
    }
    let entries = render_log(&log);
    assert!(
        entries
            .iter()
            .any(|e| e.starts_with("OK group=partner-a") && e.contains("user ")),
        "expected 'OK group=partner-a user ...', got entries: {:?}",
        entries
    );
}

#[test]
fn hit_log_without_user_still_has_group() {
    let mut log = PluginLog::new("KeyAuth");
    let meta = metadata_with_group("internal", None);
    if !meta.headers.contains_key("X-Consumer-Username") {
        log.push_fmt("OK group={}", la![meta.group.as_ref()]);
    }
    let entries = render_log(&log);
    assert!(
        entries.iter().any(|e| e == "OK group=internal"),
        "expected exact 'OK group=internal', got: {:?}",
        entries
    );
}

#[tokio::test]
async fn anonymous_path_uses_precomputed_username_forwarding_policy() {
    let mut config = create_test_config();
    config.anonymous = Some("guest".to_string());
    let auth = KeyAuth::from_config(&config);
    assert!(auth.forward_anonymous_username);

    let mut sequence = Sequence::new();
    let mut session = MockPluginSession::new();
    setup_key_headers(&mut session, None);
    session.expect_get_query_param_ref().returning(|_| None);
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Consumer-Username" && value == "guest" && value.is_sensitive())
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Anonymous-Consumer" && value == "true" && value.is_sensitive())
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    let mut log = PluginLog::new(PLUGIN_NAME);

    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert!(log.contains("Allow anon"));

    config.upstream_header_fields.clear();
    assert!(!KeyAuth::from_config(&config).forward_anonymous_username);
}

#[tokio::test]
async fn valid_key_forwards_metadata_then_hides_credential_and_masks_username() {
    const CHILD_ENV: &str = "EDGION_KEY_AUTH_MASKED_LOG_CHILD";
    const TEST_NAME: &str = "valid_key_forwards_metadata_then_hides_credential_and_masks_username";
    const TEST_MODULE: &str = "plugins::http::key_auth::plugin::tests";

    crate::test_subprocess::assert_in_subprocess_async(TEST_MODULE, TEST_NAME, CHILD_ENV, TEST_NAME, || async {
        SENSITIVE_MODE.store(1, Ordering::SeqCst);

        let mut config = create_test_config();
        config.hide_credentials = true;
        config.resolved_keys = Some(HashMap::from([(
            "valid-api-key".to_string(),
            metadata_with_group("partner-a", Some("private-user")),
        )]));
        let auth = KeyAuth::from_config(&config);
        let mut sequence = Sequence::new();
        let mut session = MockPluginSession::new();
        setup_key_headers(&mut session, Some("valid-api-key"));
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "X-Consumer-Username" && value == "private-user" && value.is_sensitive())
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_remove_request_header()
            .withf(|name| name == "X-API-Key")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new(PLUGIN_NAME);

        assert_eq!(
            auth.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(log.contains("key header"));
        assert!(log.contains("OK group=partner-a user ***"));
        let serialized = serde_json::to_string(&log).unwrap();
        assert!(!serialized.contains("valid-api-key"));
        assert!(!serialized.contains("private-user"));
    })
    .await;
}

#[tokio::test]
async fn missing_key_returns_exact_response() {
    const BODY: &[u8] = b"401 Unauthorized - Missing API key";

    let auth = KeyAuth::from_config(&create_test_config());
    let mut session = MockPluginSession::new();
    setup_key_headers(&mut session, None);
    session.expect_get_query_param_ref().returning(|_| None);
    expect_auth_response(&mut session, BODY);
    let mut log = PluginLog::new(PLUGIN_NAME);

    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("Deny no key"));
}

#[tokio::test]
async fn present_invalid_key_does_not_use_fallback_or_anonymous_access() {
    const BODY: &[u8] = b"401 Unauthorized - Invalid API key";

    let mut config = create_test_config();
    config.anonymous = Some("guest".to_string());
    config.resolved_keys = Some(HashMap::from([(
        "query-valid".to_string(),
        metadata_with_group("partner-a", None),
    )]));
    let auth = KeyAuth::from_config(&config);
    let query_reads = Arc::new(AtomicUsize::new(0));
    let query_reads_for_mock = Arc::clone(&query_reads);
    let mut session = MockPluginSession::new();
    setup_key_headers(&mut session, Some("header-invalid"));
    session.expect_get_query_param_ref().returning(move |_| {
        query_reads_for_mock.fetch_add(1, Ordering::SeqCst);
        Some("query-valid")
    });
    expect_auth_response(&mut session, BODY);
    let mut log = PluginLog::new(PLUGIN_NAME);

    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(query_reads.load(Ordering::SeqCst), 0);
    assert!(log.contains("Deny bad key"));
    let value: Value = serde_json::to_value(&log).unwrap();
    assert!(!value.to_string().contains("header-invalid"));
}

#[test]
fn borrowed_key_sources_preserve_policy_and_empty_fallback() {
    for (header, query, cookies, ctx, expected) in [
        (
            Some(b"header".as_slice()),
            Some("query"),
            vec![],
            Some("ctx"),
            Some(("header", 1)),
        ),
        (
            Some(b"".as_slice()),
            Some("query"),
            vec![],
            Some("ctx"),
            Some(("query", 2)),
        ),
        (
            Some(b"\xff".as_slice()),
            Some("query"),
            vec![],
            Some("ctx"),
            Some(("query", 2)),
        ),
        (None, Some(" "), vec![], None, Some((" ", 2))),
        (
            None,
            Some(""),
            vec![b"api_key=cookie=tail".as_slice()],
            Some("ctx"),
            Some(("cookie=tail", 3)),
        ),
        (
            None,
            None,
            vec![b"api_key=; api_key=later".as_slice()],
            Some("ctx"),
            Some(("ctx", 4)),
        ),
        (
            None,
            None,
            vec![b"API_KEY=wrong-case".as_slice()],
            Some("ctx"),
            Some(("ctx", 4)),
        ),
        (
            None,
            None,
            vec![b"opaque=\xff".as_slice(), b"api_key=later".as_slice()],
            Some("ctx"),
            Some(("ctx", 4)),
        ),
        (
            None,
            None,
            vec![b"api_key=first".as_slice(), b"opaque=\xff".as_slice()],
            Some("ctx"),
            Some(("first", 3)),
        ),
        (None, None, vec![], Some(""), None),
    ] {
        let mut auth = create_key_auth_with_keys();
        // Preserve the runtime whitelist even if a synthetic config bypasses validation.
        auth.key_sources = vec![
            KeyGet::Path,
            KeyGet::Header {
                name: "X-API-Key".to_string(),
            },
            KeyGet::Query {
                name: "api_key".to_string(),
            },
            KeyGet::Cookie {
                name: "api_key".to_string(),
            },
            KeyGet::Ctx {
                name: "api_key".to_string(),
            },
        ];
        let mut headers = http::HeaderMap::new();
        if let Some(value) = header {
            headers.append("x-api-key", http::HeaderValue::from_bytes(value).unwrap());
            headers.append("x-api-key", http::HeaderValue::from_static("ignored-duplicate"));
        }
        for cookie in cookies {
            headers.append(http::header::COOKIE, http::HeaderValue::from_bytes(cookie).unwrap());
        }
        let mut session = MockPluginSession::new();
        session.expect_key_get().times(0);
        session.expect_get_path().times(0);
        session.expect_request_header_map().return_const(headers);
        session
            .expect_get_query_param_ref()
            .withf(|name| name == "api_key")
            .return_const(query);
        session
            .expect_get_ctx_var_ref()
            .withf(|name| name == "api_key")
            .return_const(ctx);
        match (auth.extract_key(&session), expected) {
            (Some((key, source)), Some((expected, index))) => {
                assert!(matches!(key, Cow::Borrowed(_)));
                assert_eq!(key, expected);
                assert!(std::ptr::eq(source, &auth.key_sources[index]));
            }
            (None, None) => {}
            _ => panic!("unexpected credential source selection"),
        }
    }
}
