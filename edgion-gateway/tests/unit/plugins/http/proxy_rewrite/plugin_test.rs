use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::edgion_plugins::{JsonBodyRewrite, JsonLiteralValue, JsonPatchOperation};
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderValue};
use std::ptr;

fn request_headers(content_type: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers
}

fn plugin(config: &ProxyRewriteConfig) -> ProxyRewrite {
    ProxyRewrite::create(config, None).expect("valid ProxyRewrite config")
}

#[tokio::test]
async fn request_entry_preserves_current_query_and_original_path_across_rewrite() {
    for regex_mode in [false, true] {
        for query in [None, Some(""), Some("key=current&dup=1&dup=2&encoded=a%20b")] {
            let rewrite = if regex_mode {
                serde_json::json!({"regexUri": {"pattern": "^/source/(.+)$", "replacement": "/$1?own=1"}})
            } else {
                serde_json::json!({"uri": "/$arg_key?own=1"})
            };
            let mut config = rewrite;
            config["headers"] = serde_json::json!({"set": [{"name": "X-Original-Path", "value": "$uri"}]});
            let plugin = plugin(&serde_json::from_value(config).unwrap());
            let mut session = MockPluginSession::new();
            session.expect_get_path().return_const("/source/tail".to_string());
            session.expect_get_query().returning(move || query.map(str::to_owned));
            session.expect_request_query().return_const(query);
            session.expect_get_query_param_ref().returning(|name| {
                assert_eq!(name, "key");
                Some("entry")
            });
            let mut expected = if regex_mode { "/tail?own=1" } else { "/entry?own=1" }.to_string();
            if let Some(query) = query.filter(|query| !query.is_empty()) {
                expected.push('&');
                expected.push_str(query);
            }
            let mut sequence = mockall::Sequence::new();
            session
                .expect_set_upstream_uri()
                .with(mockall::predicate::eq(expected))
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_| Ok(()));
            session
                .expect_set_request_header()
                .withf(|name, value| name == "X-Original-Path" && value == "/source/tail")
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_, _| Ok(()));
            assert!(matches!(
                plugin
                    .run_request(&mut session, &mut PluginLog::new("ProxyRewrite"))
                    .await,
                PluginRunningResult::GoodNext
            ));
        }
    }
}

#[tokio::test]
async fn json_body_patch_is_ordered_and_published_once() {
    let plugin = plugin(&ProxyRewriteConfig {
        json_body: Some(JsonBodyRewrite {
            operations: vec![
                JsonPatchOperation::Test {
                    path: "/count".to_string(),
                    value: JsonLiteralValue::new(serde_json::json!(1.0)),
                    value_template: None,
                },
                JsonPatchOperation::Remove {
                    path: "/secret".to_string(),
                },
                JsonPatchOperation::Replace {
                    path: "/name".to_string(),
                    value: JsonLiteralValue::new(serde_json::json!("gateway")),
                    value_template: None,
                },
                JsonPatchOperation::Add {
                    path: "/items/-".to_string(),
                    value: JsonLiteralValue::new(serde_json::json!("tail")),
                    value_template: None,
                },
                JsonPatchOperation::Copy {
                    from: "/profile".to_string(),
                    path: "/profileCopy".to_string(),
                },
                JsonPatchOperation::Move {
                    from: "/name".to_string(),
                    path: "/displayName".to_string(),
                },
            ],
        }),
        ..Default::default()
    });
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(request_headers("application/json"));
    session.expect_declares_body().return_const(Some(true));
    session
        .expect_request_body_available_len()
        .with(mockall::predicate::eq(false))
        .return_const(Ok(88));
    session.expect_read_request_body().returning(|_| {
        Ok(Bytes::from_static(
            br#"{"name":"old","secret":"drop","count":1,"items":["head"],"profile":{"tier":"gold"}}"#,
        ))
    });
    session
            .expect_overwrite_request_body()
            .times(1)
            .withf(|body| {
                body.as_ref()
                    == br#"{"count":1,"displayName":"gateway","items":["head","tail"],"profile":{"tier":"gold"},"profileCopy":{"tier":"gold"}}"#
            })
            .returning(|_| Ok(()));
    let mut log = PluginLog::new("ProxyRewrite");

    assert!(plugin.modify_json_body(&mut session, &mut log).await.is_ok());
    assert!(log.contains("json body rewrite"));
}

fn json_session(body: &'static [u8], content_type: &'static str) -> MockPluginSession {
    json_session_with_headers(body, request_headers(content_type))
}

fn json_session_with_headers(body: &'static [u8], headers: HeaderMap) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_request_header_map().return_const(headers);
    session.expect_declares_body().return_const(Some(!body.is_empty()));
    session
        .expect_request_body_available_len()
        .with(mockall::predicate::eq(false))
        .return_const(Ok(body.len() as u64));
    session
        .expect_read_request_body()
        .with(mockall::predicate::eq(None))
        .return_once(move |_| Ok(Bytes::from_static(body)));
    session
}

#[tokio::test]
async fn failed_operation_does_not_publish_partial_patch() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "replace", "path": "/name", "value": "changed"},
                {"op": "test", "path": "/version", "value": 2}
            ]
        }
    }))
    .unwrap();
    let plugin = plugin(&config);
    let mut session = json_session(br#"{"name":"original","version":1}"#, "application/json");
    session.expect_overwrite_request_body().times(0);
    let mut log = PluginLog::new("ProxyRewrite");

    assert_eq!(
        plugin.modify_json_body(&mut session, &mut log).await,
        Err(PluginRunningResult::ErrResponse {
            status: 400,
            body: Some(Bytes::from_static(INVALID_REQUEST_BODY.as_bytes())),
        })
    );
}

#[tokio::test]
async fn value_template_is_a_bounded_json_string_and_missing_is_rejected() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "add", "path": "/tenant", "valueTemplate": "${header:X-Tenant}"}
            ]
        }
    }))
    .unwrap();
    let plugin = plugin(&config);
    let mut headers = request_headers("application/problem+json; charset=utf-8");
    headers.insert("x-tenant", http::HeaderValue::from_static("${ctx:not-expanded}"));
    let mut session = json_session_with_headers(br#"{}"#, headers);
    session
        .expect_overwrite_request_body()
        .withf(|body| body.as_ref() == br#"{"tenant":"${ctx:not-expanded}"}"#)
        .return_const(Ok(()));
    let mut log = PluginLog::new("ProxyRewrite");
    assert!(plugin.modify_json_body(&mut session, &mut log).await.is_ok());

    let mut missing = json_session(br#"{}"#, "application/json");
    missing.expect_overwrite_request_body().times(0);
    let mut log = PluginLog::new("ProxyRewrite");
    assert!(matches!(
        plugin.modify_json_body(&mut missing, &mut log).await,
        Err(PluginRunningResult::ErrResponse { status: 400, .. })
    ));
}

#[tokio::test]
async fn empty_remote_ip_is_missing_for_json_value_template() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {"operations": [{"op": "add", "path": "/client", "valueTemplate": "${remoteIp}"}]}
    }))
    .unwrap();
    let plugin = plugin(&config);
    let mut session = json_session(br#"{}"#, "application/json");
    session.expect_remote_ip().return_const(String::new());
    session.expect_overwrite_request_body().times(0);
    assert!(matches!(
        plugin
            .modify_json_body(&mut session, &mut PluginLog::new("ProxyRewrite"))
            .await,
        Err(PluginRunningResult::ErrResponse { status: 400, .. })
    ));
}

#[tokio::test]
async fn media_type_and_output_limits_fail_before_publication() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "add", "path": "/expanded", "value": "a value too large for this cap"}
            ]
        }
    }))
    .unwrap();
    let plugin = ProxyRewrite::create(
        &config,
        Some(BodyRequirement {
            max_body_size: Some("16".to_string()),
            ..BodyRequirement::fail_close()
        }),
    )
    .unwrap();
    let mut session = json_session(br#"{}"#, "application/json");
    session.expect_record_error().return_const(());
    session.expect_overwrite_request_body().times(0);
    let mut log = PluginLog::new("ProxyRewrite");
    assert!(matches!(
        plugin.modify_json_body(&mut session, &mut log).await,
        Err(PluginRunningResult::ErrResponse { status: 413, .. })
    ));

    let mut media = MockPluginSession::new();
    media
        .expect_request_header_map()
        .return_const(request_headers("text/plain"));
    media.expect_read_request_body().times(0);
    media.expect_overwrite_request_body().times(0);
    let mut log = PluginLog::new("ProxyRewrite");
    assert_eq!(
        plugin.modify_json_body(&mut media, &mut log).await,
        Err(PluginRunningResult::ErrResponse {
            status: 415,
            body: Some(Bytes::from_static(UNSUPPORTED_MEDIA_TYPE.as_bytes())),
        })
    );
}

#[test]
fn rfc_test_numeric_and_structural_equality() {
    let mut work = 0;
    assert!(rfc_json_equal(&serde_json::json!(1), &serde_json::json!(1.0), &mut work).unwrap());
    assert!(!rfc_json_equal(
        &serde_json::json!(9_007_199_254_740_992_u64),
        &serde_json::json!(9_007_199_254_740_993_u64),
        &mut work
    )
    .unwrap());
    assert!(!rfc_json_equal(
        &serde_json::json!(u64::MAX),
        &serde_json::json!(18_446_744_073_709_551_616.0),
        &mut work
    )
    .unwrap());
    assert!(rfc_json_equal(
        &serde_json::json!(10_000_000_000_000_000_000_u64),
        &serde_json::json!(1e19),
        &mut work
    )
    .unwrap());
    assert!(rfc_json_equal(
        &serde_json::json!({"a": 1, "b": [true, null]}),
        &serde_json::json!({"b": [true, null], "a": 1.0}),
        &mut work
    )
    .unwrap());
    assert!(!rfc_json_equal(&serde_json::json!([1, 2]), &serde_json::json!([2, 1]), &mut work).unwrap());
}

#[test]
fn copy_replacing_root_uses_the_final_shape_budget() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "copy", "from": "/source", "path": ""}
            ]
        }
    }))
    .unwrap();
    let plugin = plugin(&config);
    let source = "s".repeat(10_000);
    let mut document = serde_json::json!({
        "padding": "p".repeat(54_000),
        "source": source,
    });
    let operation = &plugin.config.compiled_json_body.as_ref().unwrap().operations[0];
    let session = MockPluginSession::new();
    let mut work = 0;

    plugin
        .apply_json_operation(&mut document, operation, &session, &mut work)
        .unwrap();
    assert_eq!(document, serde_json::Value::String(source));
}

#[test]
fn literal_patch_test_borrows_expected_and_preserves_work_boundaries() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {"operations": [
            {"op": "test", "path": "/target", "value": {"items": ["payload", true]}}
        ]}
    }))
    .unwrap();
    let plugin = plugin(&config);
    let operation = &plugin.config.compiled_json_body.as_ref().unwrap().operations[0];
    let CompiledJsonPatchOperation::Test { value, .. } = operation else {
        panic!("configured test operation");
    };
    let CompiledJsonPatchValue::Literal(literal) = value else {
        panic!("configured literal value");
    };
    let session = MockPluginSession::new();
    let mut work = 0;
    let expected = plugin.resolve_patch_value(value, &session, &mut work).unwrap();
    assert!(matches!(expected, Cow::Borrowed(actual) if ptr::eq(actual, literal)));
    // Four nodes, five key bytes and seven string bytes; equality visits four nodes.
    assert_eq!(work, 16);
    let original = serde_json::json!({"target": {"items": ["payload", true]}});
    for (initial_work, expected_result, final_work) in [
        (0, Ok(()), 20),
        (MAX_MUTATION_WORK - 20, Ok(()), MAX_MUTATION_WORK),
        (
            MAX_MUTATION_WORK - 19,
            Err(JsonRewriteFailure::BadRequest),
            MAX_MUTATION_WORK + 1,
        ),
        (
            MAX_MUTATION_WORK - 15,
            Err(JsonRewriteFailure::BadRequest),
            MAX_MUTATION_WORK + 1,
        ),
    ] {
        let mut document = original.clone();
        let mut work = initial_work;
        assert_eq!(
            plugin.apply_json_operation(&mut document, operation, &session, &mut work),
            expected_result
        );
        assert_eq!(work, final_work);
        assert_eq!(document, original);
    }
    let mut mismatch = serde_json::json!({"target": {"items": ["payload", false]}});
    let original = mismatch.clone();
    let mut work = 0;
    assert_eq!(
        plugin.apply_json_operation(&mut mismatch, operation, &session, &mut work),
        Err(JsonRewriteFailure::BadRequest)
    );
    assert_eq!(work, 20);
    assert_eq!(mismatch, original);
}

use edgion_resources::resources::edgion_plugins::RegexUri;

#[tokio::test]
async fn test_invalid_config() {
    use edgion_resources::resources::edgion_plugins::PluginValidation;
    let mut config = ProxyRewriteConfig {
        regex_uri: Some(RegexUri {
            pattern: r"[invalid".to_string(), // Invalid regex
            replacement: "/test".to_string(),
        }),
        ..Default::default()
    };

    // Verify config validation catches invalid regex
    let is_valid = config.precompile();
    assert!(!is_valid);
    assert!(config.validation_error().unwrap().contains("Invalid regex"));

    // Runtime publication declines an invalid configuration.
    assert!(ProxyRewrite::create(
        &ProxyRewriteConfig {
            uri: None,
            regex_uri: Some(RegexUri {
                pattern: r"[invalid".to_string(),
                replacement: "/test".to_string(),
            }),
            ..Default::default()
        },
        None,
    )
    .is_none());
}

#[test]
fn test_url_encode() {
    assert_eq!(ProxyRewrite::url_encode("hello world"), "hello%20world");
    assert_eq!(ProxyRewrite::url_encode("foo&bar"), "foo%26bar");
    assert_eq!(ProxyRewrite::url_encode("a=b"), "a%3Db");
    assert_eq!(ProxyRewrite::url_encode("test?query"), "test%3Fquery");
    assert_eq!(ProxyRewrite::url_encode("100%"), "100%25");
    assert_eq!(ProxyRewrite::url_encode("a+b"), "a%2Bb");
    assert_eq!(ProxyRewrite::url_encode("normal"), "normal");
}

#[test]
fn query_assembly_preserves_bytes_and_reuses_owned_path() {
    let plugin = plugin(&ProxyRewriteConfig::default());
    for (path, query, expected) in [
        ("/new", None, "/new"),
        ("/new?fixed=1", Some(""), "/new?fixed=1"),
        ("/new", Some("x=a%20b&x=%26"), "/new?x=a%20b&x=%26"),
        ("/new?fixed=1", Some("x=2"), "/new?fixed=1&x=2"),
        ("/new?", Some("x=2"), "/new?&x=2"),
        ("", Some("x=2"), "?x=2"),
    ] {
        let mut owned = String::with_capacity(128);
        owned.push_str(path);
        let pointer = owned.as_ptr();
        let capacity = owned.capacity();
        let result = plugin.build_uri_with_query(owned, query);
        assert_eq!(result, expected);
        assert_eq!(result.as_ptr(), pointer);
        assert_eq!(result.capacity(), capacity);
        assert_eq!(plugin.build_uri_with_query(path.to_owned(), query), expected);
    }
}

#[test]
fn dollar_restoration_transfers_plain_output_and_preserves_sentinels() {
    let mut plain = String::with_capacity(128);
    plain.push_str("/plain/$unknown/caf%C3%A9");
    let pointer = plain.as_ptr();
    let capacity = plain.capacity();
    let restored = ProxyRewrite::unescape_dollar(plain);
    assert_eq!(restored, "/plain/$unknown/caf%C3%A9");
    assert_eq!(restored.as_ptr(), pointer);
    assert_eq!(restored.capacity(), capacity);

    for (input, expected) in [
        ("", ""),
        ("\x00$uid/\x00$arg_x", "$uid/$arg_x"),
        ("a\x00b", "a\x00b"),
        ("\x00\x00$1/$plain", "\x00$1/$plain"),
    ] {
        assert_eq!(ProxyRewrite::unescape_dollar(input.to_owned()), expected);
    }
}

#[test]
fn dollar_escaping_reuses_unchanged_inputs_and_preserves_exact_output() {
    let plain = "/plain/café";
    let borrowed = ProxyRewrite::escape_dollar(Cow::Borrowed(plain));
    assert!(matches!(borrowed, Cow::Borrowed(_)));
    assert_eq!(borrowed.as_ptr(), plain.as_ptr());

    let mut owned = String::with_capacity(128);
    owned.push_str(plain);
    let pointer = owned.as_ptr();
    let capacity = owned.capacity();
    let output = ProxyRewrite::escape_dollar(Cow::Owned(owned)).into_owned();
    assert_eq!(output, plain);
    assert_eq!(output.as_ptr(), pointer);
    assert_eq!(output.capacity(), capacity);

    for (input, expected) in [
        ("", ""),
        ("$uid/$$/café", "\x00$uid/\x00$\x00$/café"),
        ("a\x00b", "a\x00b"),
        ("\x00$arg_x", "\x00\x00$arg_x"),
    ] {
        for value in [Cow::Borrowed(input), Cow::Owned(input.to_owned())] {
            let escaped = ProxyRewrite::escape_dollar(value);
            assert_eq!(escaped, expected);
            assert_eq!(ProxyRewrite::unescape_dollar(escaped.into_owned()), input);
        }
    }
}

/// `$arg_<name>` must always url-encode the (decoded) query value, in both
/// URI and header contexts. This protects header rewrite from
/// `HeaderValue::try_from` rejecting control bytes and shields downstream
/// strict HTTP parsers from non-ASCII bytes, while remaining identity for
/// ordinary ASCII values (so the common `$arg_x = user-123` UX is unchanged).
#[test]
fn test_arg_substitution_always_url_encodes() {
    let plugin = plugin(&ProxyRewriteConfig::default());

    // Mock returns decoded values.
    let mut mock_session = MockPluginSession::new();
    mock_session.expect_get_query_param_ref().returning(|name| match name {
        "user" => Some("user-123"),      // ordinary ASCII
        "token" => Some("hello\nworld"), // decoded control byte
        "id" => Some("café"),            // decoded non-ASCII byte (é = C3 A9) — must be url-encoded
        "raw" => Some("foo+bar=baz"),    // decoded reserved chars
        _ => None,
    });

    // Header context (TemplateContext::Header).
    let header_out = plugin.resolve_variables(
        "user=$arg_user;token=$arg_token;id=$arg_id;raw=$arg_raw",
        "/orig",
        &mut mock_session,
        None,
        TemplateContext::Header,
    );
    assert_eq!(
        header_out, "user=user-123;token=hello%0Aworld;id=caf%C3%A9;raw=foo%2Bbar%3Dbaz",
        "header context: $arg_<name> must url-encode"
    );

    // URI context (TemplateContext::Uri) — same outcome for $arg_ since we
    // always encode now. Other branches (capture groups, path params) still
    // honour the context; this test only pins down $arg_.
    let uri_out = plugin.resolve_variables(
        "/p?user=$arg_user&token=$arg_token&id=$arg_id&raw=$arg_raw",
        "/orig",
        &mut mock_session,
        None,
        TemplateContext::Uri,
    );
    assert_eq!(
        uri_out, "/p?user=user-123&token=hello%0Aworld&id=caf%C3%A9&raw=foo%2Bbar%3Dbaz",
        "URI context: $arg_<name> stays url-encoded"
    );
}

#[test]
fn substitution_borrows_inputs_and_preserves_missing_and_empty_values() {
    let plugin = plugin(&ProxyRewriteConfig::default());
    for context in [TemplateContext::Uri, TemplateContext::Header] {
        let mut session = MockPluginSession::new();
        session.expect_get_query_param().times(0);
        session.expect_get_path_param().times(0);
        session.expect_get_query_param_ref().returning(|name| match name {
            "empty" => Some(""),
            "value" => Some("café &"),
            _ => None,
        });
        session.expect_get_path_param_ref().returning(|name| match name {
            "empty" => Some(""),
            "value" => Some("café &"),
            _ => None,
        });
        assert_eq!(
            plugin.resolve_variables(
                "$arg_missing/$arg_empty/$arg_value/$missing/$empty/$value/$arg_value/$value",
                "/original",
                &mut session,
                None,
                context,
            ),
            "//caf%C3%A9%20%26/$missing//caf%C3%A9%20%26/caf%C3%A9%20%26/caf%C3%A9%20%26"
        );
    }
}

/// `$1..$9` (regex captures) must url-encode in Header context, mirroring
/// the `$arg_<name>` rule. Without encoding, control bytes and non-ASCII
/// values cause `HeaderValue::try_from` to silently drop the header.
#[test]
fn test_capture_substitution_url_encodes_in_header_context() {
    let plugin = plugin(&ProxyRewriteConfig::default());
    let mut mock_session = MockPluginSession::new();
    // Captures don't need session lookups, but resolve_variables takes one.
    mock_session.expect_get_query_param_ref().returning(|_| None);
    // Inert: result contains no `$` after capture substitution, so path-param
    // branch is short-circuited by `if result.contains('$')`.
    mock_session.expect_get_path_param_ref().returning(|_| None);

    // Build a fake regex::Captures by matching against a constructed string.
    // The captured text contains: a control byte, a non-ASCII (Latin-1)
    // character, a literal '%', and a reserved char.
    // Use (?s) flag so '.' matches '\n' (the control byte under test).
    let re = regex::Regex::new(r"(?s)^/(.+)$").unwrap();
    // Adversarial input pins the encoder for this function in isolation.
    // Production paths cannot contain a raw \n byte (pingora rejects request-line
    // CTL bytes upstream), so this exercises resolve_variables itself, not an
    // HTTP-realistic capture; the integration test covers the realistic case.
    let probe = "/hello\nworld-é-100%-foo&bar";
    let caps = re.captures(probe).unwrap();

    let out = plugin.resolve_variables(
        "X-Trace=$1",
        "/orig",
        &mut mock_session,
        Some(&caps),
        TemplateContext::Header,
    );
    assert_eq!(
        out, "X-Trace=hello%0Aworld-%C3%A9-100%25-foo%26bar",
        "header context: $1 must url-encode control bytes, non-ASCII, % and reserved chars"
    );
}

/// `$<name>` (path parameters) must url-encode in Header context for the
/// same reason as captures.
#[test]
fn test_path_param_substitution_url_encodes_in_header_context() {
    let plugin = plugin(&ProxyRewriteConfig::default());
    let mut mock_session = MockPluginSession::new();
    mock_session.expect_get_query_param_ref().returning(|_| None);
    // Decoded path-param value: contains non-ASCII, control byte, reserved char.
    mock_session.expect_get_path_param_ref().returning(|name| match name {
        "uid" => Some("café-foo&bar\nbaz"),
        _ => None,
    });

    let out = plugin.resolve_variables("X-User=$uid", "/orig", &mut mock_session, None, TemplateContext::Header);
    assert_eq!(
        out, "X-User=caf%C3%A9-foo%26bar%0Abaz",
        "header context: $<name> must url-encode the decoded path-param value"
    );
}

/// Pin URI-context behavior: `$1..$9` and `$<name>` continue to url-encode
/// (no behavior change in this context — guards against accidental regression
/// while we touch resolve_variables).
#[test]
fn test_capture_and_path_param_url_encode_in_uri_context() {
    let plugin = plugin(&ProxyRewriteConfig::default());
    let mut mock_session = MockPluginSession::new();
    mock_session.expect_get_query_param_ref().returning(|_| None);
    mock_session.expect_get_path_param_ref().returning(|name| match name {
        "uid" => Some("café"),
        _ => None,
    });

    // Capture path
    // Use (?s) flag so '.' matches '\n' for consistency with other capture tests.
    let re = regex::Regex::new(r"(?s)^/(.+)$").unwrap();
    let probe = "/foo&bar";
    let caps = re.captures(probe).unwrap();
    let cap_out = plugin.resolve_variables("/x/$1", "/orig", &mut mock_session, Some(&caps), TemplateContext::Uri);
    assert_eq!(cap_out, "/x/foo%26bar", "URI context: $1 stays url-encoded");

    // Path-param path
    let pp_out = plugin.resolve_variables("/u/$uid", "/orig", &mut mock_session, None, TemplateContext::Uri);
    assert_eq!(pp_out, "/u/caf%C3%A9", "URI context: $<name> stays url-encoded");
}

fn path_token_session() -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_get_path_param_ref().returning(|name| match name {
        "id" => Some("first"),
        "id2" => Some("second"),
        "id_tail" => Some("tail"),
        "empty" => Some(""),
        "encoded" => Some("café &\n$id2"),
        _ => None,
    });
    session
}

#[test]
fn path_tokens_use_exact_original_spans_in_both_contexts() {
    let plugin = plugin(&ProxyRewriteConfig::default());
    for context in [TemplateContext::Uri, TemplateContext::Header] {
        for (template, expected) in [
            ("$id/$id2/$id/$id2", "first/second/first/second"),
            ("$id2/$id/$id2/$id", "second/first/second/first"),
            ("$id$id2/$id3/$id_tail/$empty", "firstsecond/$id3/tail/"),
            ("$/$_id/$-id/$9/$arg_", "$/$_id/$-id/$9/$arg_"),
            ("$encoded/$id2", "caf%C3%A9%20%26%0A%24id2/second"),
        ] {
            assert_eq!(
                plugin.resolve_variables(template, "/orig", &mut path_token_session(), None, context),
                expected,
                "template: {template}, context: {context:?}"
            );
        }
    }
}

#[test]
fn path_tokens_preserve_injected_dollars_beside_original_tokens() {
    let plugin = plugin(&ProxyRewriteConfig::default());
    for context in [TemplateContext::Uri, TemplateContext::Header] {
        assert_eq!(
            plugin.resolve_variables(
                "$uri/$id/$id2/$uri/$id",
                "/literal/$id/$id2/$id3",
                &mut path_token_session(),
                None,
                context,
            ),
            "/literal/$id/$id2/$id3/first/second//literal/$id/$id2/$id3/first"
        );
        // Isolate final-stage reserved/escaped handling from earlier substitutions.
        assert_eq!(
            plugin.resolve_variables(
                "$uri/$id",
                "/literal/$uri/$arg_",
                &mut path_token_session(),
                None,
                context,
            ),
            "/literal/$uri/$arg_/first"
        );
    }
}

#[test]
fn path_tokens_reach_ordinary_uri_and_add_set_header_sinks() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "uri": "/$id/$id2/$id3/$empty/$encoded",
        "headers": {
            "add": [{"name": "X-Added", "value": "$id/$id2/$id/$id3/$empty/$encoded"}],
            "set": [{"name": "X-Set", "value": "$id2/$id/$uri/$id2"}]
        }
    }))
    .unwrap();
    let plugin = plugin(&config);
    let mut session = path_token_session();
    session
        .expect_set_upstream_uri()
        .with(mockall::predicate::eq(
            "/first/second/$id3//caf%C3%A9%20%26%0A%24id2?keep=a%20b",
        ))
        .times(1)
        .returning(|_| Ok(()));
    session
        .expect_append_request_header()
        .withf(|name, value| {
            name == "X-Added"
                && value == "first/second/first/$id3//caf%C3%A9%20%26%0A%24id2"
                && HeaderValue::from_str(value).is_ok()
        })
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header()
        .withf(|name, value| name == "X-Set" && value == "second/first//literal/$id/$id2/second")
        .times(1)
        .returning(|_, _| Ok(()));
    let mut log = PluginLog::new("ProxyRewrite");
    session.expect_request_query().return_const(Some("keep=a%20b"));
    plugin.rewrite_uri(&mut session, &mut log, "/literal/$id/$id2", None);
    plugin.modify_headers(&mut session, &mut log, "/literal/$id/$id2", None);
}

#[test]
fn regex_uri_raw_captures_keep_named_dollars_literal() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "regexUri": {"pattern": "^/source/(.+)$", "replacement": "/$1/$id/$id2/$id3"}
    }))
    .unwrap();
    let plugin = plugin(&config);
    let path = "/source/$id/$id2";
    let regex = Regex::new("^/source/(.+)$").unwrap();
    let captures = regex.captures(path).unwrap();
    let mut session = path_token_session();
    session
        .expect_set_upstream_uri()
        .with(mockall::predicate::eq("/$id/$id2/first/second/$id3?keep=1"))
        .times(1)
        .returning(|_| Ok(()));
    session.expect_request_query().return_const(Some("keep=1"));
    plugin.rewrite_uri(&mut session, &mut PluginLog::new("ProxyRewrite"), path, Some(&captures));
}

#[test]
fn regex_uri_capture_markers_preserve_sequential_and_unmatched_behavior() {
    // Preserve observed staging here; rewrite-01 tracks the separate grammar decision.
    for (pattern, path, replacement, expected) in [
        (
            "^/([^/]+)/([^/]+)/([^/]+)/([^/]+)/([^/]+)/([^/]+)/([^/]+)/([^/]+)/([^/]+)/([^/]+)$",
            "/a/b/c/d/e/f/g/h/i/j",
            "/$10/$1/$2",
            "/a0/a/b?keep=1",
        ),
        ("^/(a)?(b)?(c)?(d)?(e)?(f)?(g)?(h)?(i)?(j)$", "/j", "/$10", "/j?keep=1"),
        ("^/(a)?(b)$", "/b", "/$1/$2", "/$1/b?keep=1"),
    ] {
        let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "regexUri": {"pattern": pattern, "replacement": replacement}
        }))
        .unwrap();
        let plugin = plugin(&config);
        let regex = Regex::new(pattern).unwrap();
        let captures = regex.captures(path).unwrap();
        let mut session = MockPluginSession::new();
        session
            .expect_set_upstream_uri()
            .with(mockall::predicate::eq(expected))
            .times(1)
            .returning(|_| Ok(()));
        session.expect_request_query().return_const(Some("keep=1"));
        plugin.rewrite_uri(&mut session, &mut PluginLog::new("ProxyRewrite"), path, Some(&captures));
    }
}

#[tokio::test]
async fn header_host_rejects_ambiguous_or_disallowed_input_before_any_mutation() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "hostFromHeader": {"name": "X-Backend-Host", "allowRegex": "api\\.internal(?::8080)?"},
        "uri": "/rewritten"
    }))
    .unwrap();
    let plugin = plugin(&config);
    let mut cases = vec![HeaderMap::new()];
    for value in [
        "",
        "evil.internal",
        "api.internal.evil",
        "api.internal:80",
        "api.internal,evil.internal",
        "user@api.internal",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("x-backend-host", HeaderValue::from_str(value).unwrap());
        cases.push(headers);
    }
    let mut repeated = HeaderMap::new();
    repeated.append("x-backend-host", HeaderValue::from_static("api.internal"));
    repeated.append("x-backend-host", HeaderValue::from_static("api.internal"));
    cases.push(repeated);
    let mut invalid_text = HeaderMap::new();
    invalid_text.insert("x-backend-host", HeaderValue::from_bytes(&[0xff]).unwrap());
    cases.push(invalid_text);
    for headers in cases {
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);
        // No path/body read or mutation expectation: rejection must precede them.
        assert!(matches!(
            plugin
                .run_request(&mut session, &mut PluginLog::new("ProxyRewrite"))
                .await,
            PluginRunningResult::ErrResponse { status: 400, .. }
        ));
    }
}

#[tokio::test]
async fn header_host_uses_canonical_host_setter_and_propagates_failure() {
    for fails in [false, true] {
        let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "hostFromHeader": {"name": "X-Backend-Host", "allowRegex": "api\\.internal:8080"}
        }))
        .unwrap();
        let plugin = plugin(&config);
        let mut headers = HeaderMap::new();
        headers.insert("x-backend-host", HeaderValue::from_static("api.internal:8080"));
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);
        session.expect_get_path().return_const("/original".to_string());
        session
            .expect_set_upstream_host()
            .with(mockall::predicate::eq("api.internal:8080"))
            .times(1)
            .returning(move |_| if fails { Err("sealed mutation".into()) } else { Ok(()) });
        let result = plugin
            .run_request(&mut session, &mut PluginLog::new("ProxyRewrite"))
            .await;
        assert!(if fails {
            matches!(result, PluginRunningResult::ErrResponse { status: 400, .. })
        } else {
            matches!(result, PluginRunningResult::GoodNext)
        });
    }
}
