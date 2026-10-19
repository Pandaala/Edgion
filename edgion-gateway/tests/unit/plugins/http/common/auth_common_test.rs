use super::{
    escape_quoted_string, format_claim_header_value, plan_cookie_strip, resolve_claim_path,
    send_gateway_auth_error_response, set_claims_headers, strip_cookie, strip_query_param,
    AggregateHeaderBudgetExceeded, Claims, CookieStripPlan,
};
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use mockall::Sequence;
use parking_lot::Mutex;
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use edgion_resources::resources::edgion_plugins::compile_claim_header_projection;

#[test]
fn claim_header_array_formatting_is_bounded_and_preserves_scalar_order() {
    let value = json!(["alpha", {"ignored": true}, 7, false]);
    let expected = "alpha,7,false";

    assert_eq!(
        format_claim_header_value(&value, expected.len(), expected.len())
            .unwrap()
            .as_deref(),
        Some(expected)
    );
    assert!(format_claim_header_value(&value, expected.len() - 1, usize::MAX)
        .unwrap()
        .is_none());
}

#[test]
fn claim_header_array_preserves_empty_comma_utf8_and_unsafe_semantics() {
    let value = json!(["", "a,b", "é", 7, false, {"ignored": true}]);
    let expected = ",a,b,é,7,false";
    assert_eq!(
        format_claim_header_value(&value, expected.len(), expected.len())
            .unwrap()
            .as_deref(),
        Some(expected)
    );

    let unsafe_value = json!(["safe", "bad\nvalue"]);
    assert!(
        format_claim_header_value(&unsafe_value, usize::MAX, 1)
            .unwrap()
            .is_none(),
        "unsafe values must skip rather than consume aggregate order"
    );
}

#[test]
fn formatting_distinguishes_per_value_skip_from_aggregate_stop_before_allocation() {
    let value = json!(["abc", "def"]);
    assert!(
        format_claim_header_value(&value, 6, usize::MAX).unwrap().is_none(),
        "per-value overflow must skip the candidate"
    );
    assert!(matches!(
        format_claim_header_value(&value, 7, 6),
        Err(AggregateHeaderBudgetExceeded)
    ));
}

#[test]
fn claim_header_scalar_value_enforces_exact_byte_cap() {
    let value = json!("é");
    assert_eq!(format_claim_header_value(&value, 2, 2).unwrap().as_deref(), Some("é"));
    assert!(format_claim_header_value(&value, 1, usize::MAX).unwrap().is_none());
}

#[test]
fn failed_header_write_does_not_consume_aggregate_budget() {
    let mapping = HashMap::from([
        ("first".to_string(), "x-a".to_string()),
        ("second".to_string(), "x-b".to_string()),
    ]);
    let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
    let claims = json!({"first": "1", "second": "2"});
    let mut session = MockPluginSession::new();
    let mut sequence = Sequence::new();

    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "x-a" && value == "1" && value.is_sensitive())
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Err(std::io::Error::other("injected write failure").into()));
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "x-b" && value == "2" && value.is_sensitive())
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));

    set_claims_headers(&mut session, &claims, &plan, 1, 4);
}

#[test]
fn aggregate_cap_uses_the_plan_order_at_exact_boundary() {
    let mapping = HashMap::from([
        ("second".to_string(), "x-b".to_string()),
        ("first".to_string(), "x-a".to_string()),
    ]);
    let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
    let claims = json!({"first": "1", "second": "2"});
    let mut session = MockPluginSession::new();

    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "x-a" && value == "1" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));

    set_claims_headers(&mut session, &claims, &plan, 1, 4);
}

#[test]
fn shuffled_mapping_construction_and_execution_produce_identical_writes() {
    let source = [
        ("claim_c", "X-C", "c"),
        ("claim_a", "X-A", "a"),
        ("claim_b", "X-B", "b"),
    ];

    for rotation in 0..source.len() {
        let mut mapping = HashMap::new();
        let mut claims = serde_json::Map::new();
        for offset in 0..source.len() {
            let (claim, header, value) = source[(rotation + offset) % source.len()];
            mapping.insert(claim.to_string(), header.to_string());
            claims.insert(claim.to_string(), json!(value));
        }

        let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
        let writes = Arc::new(Mutex::new(Vec::new()));
        let captured = writes.clone();
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header_value()
            .times(3)
            .returning(move |name, value| {
                assert!(value.is_sensitive());
                captured
                    .lock()
                    .push((name.to_string(), value.to_str().unwrap().to_string()));
                Ok(())
            });

        set_claims_headers(&mut session, &serde_json::Value::Object(claims), &plan, 8, 64);
        assert_eq!(
            *writes.lock(),
            vec![
                ("x-a".to_string(), "a".to_string()),
                ("x-b".to_string(), "b".to_string()),
                ("x-c".to_string(), "c".to_string()),
            ]
        );
    }
}

#[test]
fn aggregate_cap_rejects_the_first_candidate_at_plus_one() {
    let mapping = HashMap::from([("first".to_string(), "x-a".to_string())]);
    let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
    let claims = json!({"first": "1"});
    let mut session = MockPluginSession::new();
    session.expect_set_request_header_value().never();

    set_claims_headers(&mut session, &claims, &plan, 1, 3);
}

#[test]
fn invalid_header_value_does_not_consume_aggregate_budget() {
    let mapping = HashMap::from([
        ("unsafe".to_string(), "x-a".to_string()),
        ("safe".to_string(), "x-b".to_string()),
    ]);
    let plan = compile_claim_header_projection(Some(&mapping)).unwrap();
    let claims = json!({"unsafe": "bad\u{7f}", "safe": "2"});
    let mut session = MockPluginSession::new();

    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "x-b" && value == "2" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));

    set_claims_headers(&mut session, &claims, &plan, 4, 7);
}

#[tokio::test]
async fn auth_realm_challenges_escape_data_and_preserve_response_contract() {
    for scheme in ["Basic", "ApiKey", "Bearer", "hmac", "ldap", "TLSCert"] {
        for error_code in [None, Some("invalid_token")] {
            for (realm, escaped) in [
                ("edgion", "edgion"),
                ("", ""),
                ("Partner \"API\"", "Partner \\\"API\\\""),
                ("path\\", "path\\\\"),
                ("a, b=c", "a, b=c"),
                ("café €", "café €"),
            ] {
                let mut session = MockPluginSession::new();
                let mut sequence = Sequence::new();
                let mut expected = format!("{scheme} realm=\"{escaped}\"");
                if error_code.is_some() {
                    expected.push_str(", error=\"invalid_token\", error_description=\"Denied \\\"token\\\"\\\\\"");
                }
                session
                    .expect_write_response_header()
                    .withf(move |response, eos| {
                        !eos && response.status.as_u16() == 401
                            && response.headers.get("content-type").unwrap() == "text/plain"
                            && response.headers.get("www-authenticate").unwrap().as_bytes() == expected.as_bytes()
                    })
                    .times(1)
                    .in_sequence(&mut sequence)
                    .returning(|_, _| Ok(()));
                session
                    .expect_write_response_body()
                    .withf(|body, eos| *eos && body.as_deref() == Some(b"401 Denied \"token\"\\\r\n".as_slice()))
                    .times(1)
                    .in_sequence(&mut sequence)
                    .returning(|_, _| Ok(()));
                session
                    .expect_shutdown()
                    .times(1)
                    .in_sequence(&mut sequence)
                    .returning(|| {});
                send_gateway_auth_error_response(
                    &mut session,
                    401,
                    scheme,
                    realm,
                    error_code,
                    "Denied \"token\"\\\r\n",
                )
                .await
                .unwrap();
            }
        }
    }
}

#[tokio::test]
async fn auth_realm_controls_fail_before_any_response_is_written() {
    for byte in (0..=0x1f).chain(std::iter::once(0x7f)) {
        for error_code in [None, Some("invalid_token")] {
            let mut session = MockPluginSession::new();
            session.expect_write_response_header().never();
            session.expect_write_response_body().never();
            session.expect_shutdown().never();
            let realm = format!("private-realm{}suffix", char::from(byte));
            let error = send_gateway_auth_error_response(&mut session, 401, "Bearer", &realm, error_code, "Denied")
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "realm must not contain ASCII control characters (including tabs and DEL)"
            );
        }
    }
}

#[test]
fn auth_realm_plain_values_do_not_allocate_an_escaped_copy() {
    assert!(matches!(escape_quoted_string("API Gateway café"), Cow::Borrowed(_)));
}

#[test]
fn test_escape_quoted_string() {
    // Plain text is unchanged.
    assert_eq!(escape_quoted_string("Token expired"), "Token expired");
    // Backslash and double-quote are backslash-escaped.
    assert_eq!(escape_quoted_string(r#"a"b\c"#), r#"a\"b\\c"#);
    // Control characters (CR/LF/NUL) are dropped to prevent header injection.
    assert_eq!(escape_quoted_string("a\r\nb\0c"), "abc");
}

#[test]
fn test_resolve_claim_path_dot_notation() {
    let claims = json!({
        "sub": "u1",
        "realm_access": {
            "roles": ["admin", "dev"]
        }
    });

    let sub = resolve_claim_path(&claims, "sub").and_then(|v| v.as_str());
    assert_eq!(sub, Some("u1"));

    let roles = resolve_claim_path(&claims, "realm_access.roles")
        .and_then(|v| v.as_array())
        .map(|v| v.len());
    assert_eq!(roles, Some(2));

    assert!(resolve_claim_path(&claims, "realm_access.missing").is_none());
}

#[test]
fn test_claims_to_value_includes_standard_fields() {
    let claims = Claims {
        iss: Some("https://issuer.example.com".to_string()),
        sub: Some("user-1".to_string()),
        aud: Some(json!(["api"])),
        exp: Some(123),
        iat: Some(100),
        nbf: None,
        extra: json!({"email": "u1@example.com"}),
    };

    let value = claims.to_value();
    assert_eq!(
        value.get("iss").and_then(|v| v.as_str()),
        Some("https://issuer.example.com")
    );
    assert_eq!(value.get("sub").and_then(|v| v.as_str()), Some("user-1"));
    assert_eq!(value.get("exp").and_then(|v| v.as_u64()), Some(123));
    assert_eq!(value.get("email").and_then(|v| v.as_str()), Some("u1@example.com"));
}

#[test]
fn strip_query_param_removes_named_keeps_others() {
    let mut session = MockPluginSession::new();
    session
        .expect_request_query()
        .return_const(Some("api_key=secret&foo=bar"));
    session
        .expect_set_request_query()
        .withf(|query| query == "foo=bar")
        .times(1)
        .returning(|_| Ok(()));

    strip_query_param(&mut session, "api_key");
}

#[test]
fn strip_query_param_only_param_clears_query() {
    let mut session = MockPluginSession::new();
    session.expect_request_query().return_const(Some("api_key=secret"));
    session
        .expect_set_request_query()
        .withf(|query| query.is_empty())
        .times(1)
        .returning(|_| Ok(()));

    strip_query_param(&mut session, "api_key");
}

#[test]
fn strip_query_param_without_match_leaves_query_untouched() {
    let mut session = MockPluginSession::new();
    session.expect_request_query().return_const(Some("a=1&&b=2&"));
    session.expect_set_request_query().times(0);

    strip_query_param(&mut session, "api_key");
}

#[test]
fn strip_cookie_removes_named_keeps_others() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("Cookie", "api_key=secret; other=1")]);
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "Cookie" && value == "other=1" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));

    strip_cookie(&mut session, "api_key");
}

#[test]
fn strip_cookie_only_cookie_removes_header() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("Cookie", "api_key=secret")]);
    session
        .expect_remove_request_header()
        .withf(|name| name == "Cookie")
        .times(1)
        .returning(|_| Ok(()));

    strip_cookie(&mut session, "api_key");
}

#[test]
fn strip_cookie_merges_multiple_cookie_lines() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("cookie", "api_key=secret; a=1"), ("cookie", "b=2")]);
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "Cookie" && value == "a=1; b=2" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));

    strip_cookie(&mut session, "api_key");
}

// ── plan_cookie_strip: pure logic, no session mock ───────────────────────
//
// These cover the parsing / merging / edge-case rules directly. The mock
// tests above only need to prove the session wiring (read -> decide ->
// rewrite), not re-derive the semantics.

fn plan(lines: &[&str], names: &[&str]) -> CookieStripPlan {
    plan_cookie_strip(lines.iter().copied(), names)
}

#[test]
fn plan_cookie_strip_reports_absent_without_any_cookie_line() {
    assert_eq!(plan(&[], &["api_key"]), CookieStripPlan::Absent);
}

#[test]
fn plan_cookie_strip_removes_header_when_nothing_survives() {
    assert_eq!(plan(&["api_key=secret"], &["api_key"]), CookieStripPlan::RemoveHeader);
    // An empty Cookie line is still a present header with no survivors.
    assert_eq!(plan(&[""], &["api_key"]), CookieStripPlan::RemoveHeader);
    assert_eq!(plan(&["  ;  ; "], &["api_key"]), CookieStripPlan::RemoveHeader);
}

#[test]
fn plan_cookie_strip_keeps_unnamed_pairs_and_trims_whitespace() {
    assert_eq!(
        plan(&["api_key=secret;  other=1 ; third=3"], &["api_key"]),
        CookieStripPlan::Replace("other=1; third=3".to_string())
    );
}

#[test]
fn plan_cookie_strip_merges_multiple_cookie_lines_into_one() {
    assert_eq!(
        plan(&["a=1; api_key=secret", "b=2", "api_key=again; c=3"], &["api_key"]),
        CookieStripPlan::Replace("a=1; b=2; c=3".to_string())
    );
}

#[test]
fn plan_cookie_strip_removes_every_named_cookie_in_one_pass() {
    // Equivalent to stripping each name in sequence, but with a single scan.
    assert_eq!(
        plan(
            &[
                "edgion_oidc_session=secret; app=keep",
                "edgion_oidc_state=state; theme=dark"
            ],
            &["edgion_oidc_session", "edgion_oidc_state"],
        ),
        CookieStripPlan::Replace("app=keep; theme=dark".to_string())
    );
}

#[test]
fn plan_cookie_strip_matches_the_raw_key_before_the_first_equals() {
    // Mirrors `get_cookie`: the key is the raw text before the first '=',
    // so a percent-encoded or differently-cased name is a different cookie.
    assert_eq!(
        plan(&["API_KEY=1; api%5Fkey=2; api_key=3"], &["api_key"]),
        CookieStripPlan::Replace("API_KEY=1; api%5Fkey=2".to_string())
    );
    // A valueless pair keeps its whole text when it is not a match.
    assert_eq!(
        plan(&["flag; api_key=3"], &["api_key"]),
        CookieStripPlan::Replace("flag".to_string())
    );
    // ...and is stripped when the bare name matches.
    assert_eq!(
        plan(&["api_key; x=1"], &["api_key"]),
        CookieStripPlan::Replace("x=1".to_string())
    );
}

#[test]
fn plan_cookie_strip_with_no_names_keeps_everything() {
    assert_eq!(
        plan(&["a=1; b=2"], &[]),
        CookieStripPlan::Replace("a=1; b=2".to_string())
    );
}

#[test]
fn query_credential_strip_preserves_live_raw_query_boundaries() {
    for (query, name, expected) in [
        (None, "api_key", None),
        (Some(""), "api_key", None),
        (Some("&&a=1&&b=2&"), "api_key", None),
        (
            Some("api%5Fkey=secret&x=a%26b&api_key=again&&flag&x=+&"),
            "api_key",
            Some("x=a%26b&flag&x=+"),
        ),
        (Some("api_key=one&api_key=two"), "api_key", Some("")),
        (Some("api_key&keep==value"), "api_key", Some("keep==value")),
        (
            Some("bad%ZZ=one&api_key=secret&%61=two"),
            "api_key",
            Some("bad%ZZ=one&%61=two"),
        ),
        (Some("api+key=secret&keep=1"), "api key", Some("keep=1")),
        (Some("=secret&&keep=1"), "", Some("keep=1")),
        (Some("api%5Fkey=secret&keep=1"), "api%5Fkey", None),
        (Some("api+key=secret&keep=1"), "api+key", None),
        (Some("api%2Bkey=secret&keep=1"), "api+key", Some("keep=1")),
        (Some("bad%+x=secret&keep=1"), "bad%+x", Some("keep=1")),
        (Some("caf%E9=secret&keep=1"), "café", Some("keep=1")),
        (Some("café=secret&keep=1"), "café", Some("keep=1")),
    ] {
        for fail_setter in [false, true] {
            let mut session = MockPluginSession::new();
            session.expect_get_query().times(0);
            session.expect_request_query().times(1).return_const(query);
            if let Some(expected) = expected {
                session
                    .expect_set_request_query()
                    .withf(move |actual| actual == expected)
                    .times(1)
                    .returning(move |_| {
                        if fail_setter {
                            Err(std::io::Error::other("query update rejected").into())
                        } else {
                            Ok(())
                        }
                    });
            } else {
                session.expect_set_request_query().times(0);
            }
            strip_query_param(&mut session, name);
        }
    }
}

#[test]
fn plain_query_key_comparison_does_not_allocate_key_sized_storage() {
    fn measure(query: &'static str) -> (usize, usize) {
        let mut session = MockPluginSession::new();
        session.expect_get_query().times(0);
        session.expect_request_query().times(1).return_const(Some(query));
        session.expect_set_request_query().times(0);
        let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            strip_query_param(&mut session, "absent");
        });
        (stats.calls, stats.requested_bytes)
    }
    let short = measure("x=1");
    let long = measure("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_long_key=1");
    assert_eq!(short, long, "plain key length must not change temporary allocation");
}

#[test]
fn cookie_strip_output_supports_owned_header_conversion() {
    let long = format!("keep={}", "x".repeat(4096));
    for lines in [
        vec!["api_key=secret; keep=1"],
        vec!["a=1; api_key=secret", "b=2; flag"],
        vec!["keep=café; api_key=secret"],
        vec!["keep=a\tb; api_key=secret"],
        vec!["keep=a\rb; api_key=secret"],
        vec!["keep=a\nb; api_key=secret"],
        vec!["keep=a\0b; api_key=secret"],
        vec!["keep=a\u{7f}b; api_key=secret"],
        vec![long.as_str()],
    ] {
        let CookieStripPlan::Replace(value) = plan(&lines, &["api_key"]) else {
            panic!("fixture must retain a cookie");
        };
        assert_eq!(value.capacity(), value.len());
        let input_ptr = value.as_ptr();
        let borrowed = http::HeaderValue::from_str(&value);
        let (owned, stats) =
            crate::test_allocation_counter::count_allocation_stats(|| http::HeaderValue::try_from(value));
        assert_eq!(owned.is_ok(), borrowed.is_ok());
        if let (Ok(owned), Ok(borrowed)) = (owned, borrowed) {
            assert_eq!(owned, borrowed);
            assert_eq!(owned.as_bytes().as_ptr(), input_ptr);
        }
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}

#[test]
fn sensitive_typed_header_write_preserves_storage_and_setter_result() {
    for fail_setter in [false, true] {
        let value = http::HeaderValue::try_from("keep=private-cookie".to_owned()).unwrap();
        let input_ptr = value.as_bytes().as_ptr() as usize;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header_value()
            .withf(move |name, value| {
                name == "Cookie"
                    && value == "keep=private-cookie"
                    && value.is_sensitive()
                    && value.as_bytes().as_ptr() as usize == input_ptr
            })
            .times(1)
            .returning(move |_, _| {
                if fail_setter {
                    Err(std::io::Error::other("header update rejected").into())
                } else {
                    Ok(())
                }
            });
        let result = super::set_sensitive_request_header_value(&mut session, "Cookie", value);
        assert_eq!(result.is_err(), fail_setter);
    }
}

#[test]
fn auth_challenge_exact_capacity_preserves_bytes_and_header_conversion() {
    let long = "x".repeat(4096);
    for scheme in ["Basic", "ApiKey", "Bearer", "hmac", "ldap", "TLSCert", "", "bad\rname"] {
        for realm in [
            "edgion",
            "",
            "Partner \"API\"",
            "path\\",
            "a, b=c",
            "café €",
            long.as_str(),
        ] {
            let escaped_realm = escape_quoted_string(realm);
            for code in [None, Some(""), Some("invalid_token"), Some("a\"b"), Some("bad\0code")] {
                for body in ["Denied", "", "a\"b\\c", "a\r\n\tb", "café €", long.as_str()] {
                    let expected = match code {
                        Some(code) => format!(
                            "{} realm=\"{}\", error=\"{}\", error_description=\"{}\"",
                            scheme,
                            escaped_realm,
                            code,
                            escape_quoted_string(body)
                        ),
                        None => format!("{} realm=\"{}\"", scheme, escaped_realm),
                    };
                    let value = super::build_auth_challenge(scheme, &escaped_realm, code, body);
                    assert_eq!(value, expected);
                    assert_eq!(value.capacity(), value.len());
                    let input_ptr = value.as_ptr();
                    let borrowed = http::HeaderValue::from_str(&value);
                    let (owned, stats) =
                        crate::test_allocation_counter::count_allocation_stats(|| http::HeaderValue::try_from(value));
                    assert_eq!(owned.is_ok(), borrowed.is_ok());
                    if let (Ok(owned), Ok(borrowed)) = (owned, borrowed) {
                        assert_eq!(owned, borrowed);
                        assert_eq!(owned.as_bytes().as_ptr(), input_ptr);
                    }
                    assert_eq!(stats.calls, 0);
                    assert_eq!(stats.requested_bytes, 0);
                }
            }
        }
    }
}

#[test]
fn auth_challenge_plain_fields_need_one_exact_output_allocation() {
    for (code, body) in [(None, "ignored\"\\\r\n"), (Some("invalid_token"), "Denied café")] {
        let (challenge, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            super::build_auth_challenge("Bearer", "edgion", code, body)
        });
        assert_eq!(stats.calls, 1);
        assert_eq!(stats.requested_bytes, challenge.len());
    }
}

#[test]
fn auth_error_body_preserves_bytes_with_one_exact_allocation() {
    let long = "x".repeat(4096);
    let mut unexpected_allocation = None;
    for status in [0u16, 1, 9, 10, 99, 1000, 9999, 10000, u16::MAX]
        .into_iter()
        .chain(100..=999)
    {
        for body in ["", "Denied", "a\"b\\c", "a\r\n\tb\0", "café €", long.as_str()] {
            let expected = format!("{status} {body}");
            let (actual, stats) =
                crate::test_allocation_counter::count_allocation_stats(|| super::build_auth_error_body(status, body));
            assert_eq!(actual.as_ref(), expected.as_bytes());
            if stats.calls != 1 || stats.requested_bytes != actual.len() {
                unexpected_allocation.get_or_insert((
                    status,
                    body.len(),
                    stats.calls,
                    stats.requested_bytes,
                    actual.len(),
                ));
            }
        }
    }
    assert_eq!(
        unexpected_allocation, None,
        "body construction must allocate exactly the output bytes once"
    );
}

#[tokio::test]
async fn auth_error_response_stops_at_the_first_write_failure() {
    for fail_header in [false, true] {
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_write_response_header()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |_, _| {
                if fail_header {
                    Err(std::io::Error::other("header rejected").into())
                } else {
                    Ok(())
                }
            });
        if fail_header {
            session.expect_write_response_body().times(0);
        } else {
            session
                .expect_write_response_body()
                .withf(|body, eos| *eos && body.as_deref() == Some(b"401 Denied".as_slice()))
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_, _| Err(std::io::Error::other("body rejected").into()));
        }
        session.expect_shutdown().times(0);
        let error = send_gateway_auth_error_response(&mut session, 401, "Bearer", "edgion", None, "Denied")
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            if fail_header {
                "header rejected"
            } else {
                "body rejected"
            }
        );
    }
}
