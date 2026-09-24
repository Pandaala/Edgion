use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::edgion_plugins::{JsonBodyRewrite, JsonLiteralValue, JsonPatchOperation};
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderValue};

fn request_headers(content_type: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers
}

fn plugin(config: &ProxyRewriteConfig) -> ProxyRewrite {
    ProxyRewrite::create(config, None).expect("valid ProxyRewrite config")
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
    mock_session.expect_get_query_param().returning(|name| match name {
        "user" => Some("user-123".to_string()),      // ordinary ASCII
        "token" => Some("hello\nworld".to_string()), // decoded control byte
        "id" => Some("café".to_string()),            // decoded non-ASCII byte (é = C3 A9) — must be url-encoded
        "raw" => Some("foo+bar=baz".to_string()),    // decoded reserved chars
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

/// `$1..$9` (regex captures) must url-encode in Header context, mirroring
/// the `$arg_<name>` rule. Without encoding, control bytes and non-ASCII
/// values cause `HeaderValue::try_from` to silently drop the header.
#[test]
fn test_capture_substitution_url_encodes_in_header_context() {
    let plugin = plugin(&ProxyRewriteConfig::default());
    let mut mock_session = MockPluginSession::new();
    // Captures don't need session lookups, but resolve_variables takes one.
    mock_session.expect_get_query_param().returning(|_| None);
    // Inert: result contains no `$` after capture substitution, so path-param
    // branch is short-circuited by `if result.contains('$')`.
    mock_session.expect_get_path_param().returning(|_| None);

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
    mock_session.expect_get_query_param().returning(|_| None);
    // Decoded path-param value: contains non-ASCII, control byte, reserved char.
    mock_session.expect_get_path_param().returning(|name| match name {
        "uid" => Some("café-foo&bar\nbaz".to_string()),
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
    mock_session.expect_get_query_param().returning(|_| None);
    mock_session.expect_get_path_param().returning(|name| match name {
        "uid" => Some("café".to_string()),
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
