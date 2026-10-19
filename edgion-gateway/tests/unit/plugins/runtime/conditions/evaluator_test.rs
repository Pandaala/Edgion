use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use crate::request_body::BodyReadError;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use edgion_resources::common::KeyGet;
use edgion_resources::resources::edgion_plugins::conditions::{MtlsField, SanType};
use edgion_resources::resources::edgion_plugins::{HmacCredential, HmacMatchConfig};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Create a mock session with key_get that returns values based on the key
fn create_mock_session_with_key_get(key_values: Vec<(KeyGet, Option<String>)>) -> MockPluginSession {
    let mut mock = MockPluginSession::new();

    let mut headers = http::HeaderMap::new();
    for (key, value) in &key_values {
        if let (KeyGet::Header { name }, Some(value)) = (key, value) {
            headers.insert(
                http::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                http::HeaderValue::from_str(value).expect("valid header value"),
            );
        }
    }
    mock.expect_request_header_map().return_const(headers);
    let borrowed_value = |target: KeyGet| {
        key_values
            .iter()
            .find(|(key, _)| *key == target)
            .and_then(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    mock.expect_get_path().return_const(borrowed_value(KeyGet::Path));
    mock.expect_get_request_path()
        .return_const(borrowed_value(KeyGet::RequestPath));
    mock.expect_get_method().return_const(borrowed_value(KeyGet::Method));
    mock.expect_remote_ip().return_const(borrowed_value(KeyGet::RemoteIp));
    mock.expect_direct_peer_ip()
        .return_const(borrowed_value(KeyGet::DirectPeerIp));

    // Convert to owned map
    let values_map: std::collections::HashMap<String, Option<String>> =
        key_values.into_iter().map(|(k, v)| (format!("{:?}", k), v)).collect();

    mock.expect_key_get().returning(move |key| {
        let key_str = format!("{:?}", key);
        values_map.get(&key_str).cloned().flatten()
    });

    mock
}

#[test]
fn borrowed_condition_inputs_recheck_each_chunk_without_owned_key_get() {
    let conditions = CompiledPluginConditions::new(PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![
            key_match("X-Mode", &["enabled"]),
            Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Method,
                core: core_values(vec!["GET"]),
                negate: false,
                descriptor: Box::from(""),
            }),
        ])),
    });
    for (header, expected) in [("enabled", EvaluationResult::Run), ("disabled", EvaluationResult::Skip)] {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-mode", http::HeaderValue::from_str(header).unwrap());
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);
        session.expect_get_method().return_const("GET".to_string());
        session.expect_key_get().never();
        assert_eq!(conditions.evaluate(&session), expected);
        let (result, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            std::hint::black_box(conditions.evaluate(&session))
        });
        assert_eq!(result, expected);
        assert_eq!(allocations.calls, 0);
    }
}

#[test]
fn borrowed_header_condition_keeps_invalid_utf8_missing() {
    let mut headers = http::HeaderMap::new();
    headers.insert("x-mode", http::HeaderValue::from_bytes(&[0xff]).unwrap());
    let mut session = MockPluginSession::new();
    session.expect_request_header_map().return_const(headers);
    session.expect_key_get().never();
    let condition = KeyExistCondition {
        key: KeyGet::Header {
            name: "X-Mode".to_string(),
        },
        negate: false,
        descriptor: Box::from(""),
    };
    assert!(!condition.evaluate(&session));
}

#[test]
fn borrowed_cookie_condition_uses_live_canonical_parser_without_allocating() {
    let mut headers = http::HeaderMap::new();
    headers.append(
        http::header::COOKIE,
        http::HeaderValue::from_static("other=no; sid=abc"),
    );
    let mut session = MockPluginSession::new();
    session.expect_request_header_map().return_const(headers);
    session.expect_key_get().never();
    let key = KeyGet::Cookie {
        name: "sid".to_string(),
    };
    assert_eq!(key_value(&session, &key).as_deref(), Some("abc"));
    let (value, allocations) =
        crate::test_allocation_counter::count_allocation_stats(|| std::hint::black_box(key_value(&session, &key)));
    assert_eq!(value.as_deref(), Some("abc"));
    assert_eq!(allocations.calls, 0);
}

#[test]
fn borrowed_ip_keys_keep_direct_and_resolved_identities_distinct() {
    let mut session = MockPluginSession::new();
    session.expect_direct_peer_ip().return_const("10.0.0.1".to_string());
    session.expect_remote_ip().return_const("203.0.113.2".to_string());
    session.expect_key_get().never();
    assert_eq!(key_value(&session, &KeyGet::DirectPeerIp).as_deref(), Some("10.0.0.1"));
    assert_eq!(key_value(&session, &KeyGet::RemoteIp).as_deref(), Some("203.0.113.2"));
}

fn fixed_utc(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid fixed timestamp")
        .with_timezone(&Utc)
}

fn compile_time_range(condition: &TimeRangeCondition) -> CompiledCondition {
    CompiledCondition::compile(
        &Condition::TimeRange(condition.clone()),
        "conditions.allOf[0]".to_string(),
        None,
        None,
    )
}

fn compiled_time_range(compiled: &CompiledCondition) -> &CompiledTimeRangeCondition {
    let CompiledPredicate::TimeRange(condition) = &compiled.predicate else {
        panic!("timeRange must use the compiled time predicate");
    };
    condition
}

#[test]
fn compiled_time_range_stores_parsed_utc_bounds() {
    let condition = TimeRangeCondition {
        after: Some("2025-01-01T02:00:00+02:00".to_string()),
        before: Some("2025-01-01T03:00:00+02:00".to_string()),
        negate: false,
        descriptor: Box::from(""),
    };
    let compiled = compile_time_range(&condition);
    let time_range = compiled_time_range(&compiled);

    assert_eq!(time_range.after.as_ref(), Some(&fixed_utc("2025-01-01T00:00:00Z")));
    assert_eq!(time_range.before.as_ref(), Some(&fixed_utc("2025-01-01T01:00:00Z")));
    assert_eq!(compiled.descriptor(), "conditions.allOf[0]:timeRange");
}

#[test]
fn time_range_matches_at_is_after_inclusive_and_before_exclusive() {
    let condition = TimeRangeCondition {
        after: Some("2025-01-01T00:00:00Z".to_string()),
        before: Some("2025-01-01T01:00:00Z".to_string()),
        negate: false,
        descriptor: Box::from(""),
    };
    let compiled = compile_time_range(&condition);
    let time_range = compiled_time_range(&compiled);

    assert!(!time_range.evaluate_at(&fixed_utc("2024-12-31T23:59:59Z")));
    assert!(time_range.evaluate_at(&fixed_utc("2025-01-01T00:00:00Z")));
    assert!(time_range.evaluate_at(&fixed_utc("2025-01-01T00:30:00Z")));
    assert!(!time_range.evaluate_at(&fixed_utc("2025-01-01T01:00:00Z")));
}

#[test]
fn compiled_time_range_evaluate_at_applies_negation_once() {
    let condition = TimeRangeCondition {
        after: Some("2025-01-01T00:00:00Z".to_string()),
        before: Some("2025-01-01T01:00:00Z".to_string()),
        negate: true,
        descriptor: Box::from(""),
    };
    let compiled = compile_time_range(&condition);
    let time_range = compiled_time_range(&compiled);

    assert!(!time_range.evaluate_at(&fixed_utc("2025-01-01T00:30:00Z")));
    assert!(time_range.evaluate_at(&fixed_utc("2025-01-01T01:00:00Z")));
}

#[test]
fn compiled_time_range_treats_invalid_bound_as_absent_defensively() {
    let condition = TimeRangeCondition {
        after: Some("not-a-timestamp".to_string()),
        before: Some("2025-01-01T01:00:00Z".to_string()),
        negate: false,
        descriptor: Box::from(""),
    };
    assert!(condition.validate().is_err());

    let compiled = compile_time_range(&condition);
    let time_range = compiled_time_range(&compiled);
    assert!(time_range.after.is_none());
    assert_eq!(time_range.before.as_ref(), Some(&fixed_utc("2025-01-01T01:00:00Z")));
    assert!(time_range.evaluate_at(&fixed_utc("2025-01-01T00:30:00Z")));
    assert!(!time_range.evaluate_at(&fixed_utc("2025-01-01T01:00:00Z")));
}

#[test]
fn compiled_header_hmac_reuses_one_credentials_arc_across_evaluations() {
    let condition = Condition::HmacAuth(HmacMatchConfig {
        resolved_credentials: Some(std::collections::HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: b"header-hmac-secret-at-least-32-bytes".to_vec(),
                headers: std::collections::HashMap::new(),
                group: "default".to_string(),
            },
        )])),
        descriptor: Box::from("0:hmacAuth"),
        ..Default::default()
    });
    let compiled = CompiledCondition::compile(&condition, "conditions.allOf[0]".to_string(), None, None);
    let credentials_ptr = compiled
        .hmac_credentials_ptr()
        .expect("header-only HMAC must prebuild one verifier");
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[]);

    assert_eq!(compiled.evaluate_sync(&session), MatchState::False);
    assert_eq!(compiled.evaluate_sync(&session), MatchState::False);
    assert_eq!(compiled.hmac_credentials_ptr(), Some(credentials_ptr));
}

fn unresolved_secret_condition() -> Condition {
    Condition::SecretMatch(
        edgion_resources::resources::edgion_plugins::conditions::SecretMatchCondition {
            key: KeyGet::Header {
                name: "X-Secret".to_string(),
            },
            secret_key: "token".to_string(),
            secret_ref: edgion_resources::resources::common::SecretObjectReference {
                name: "missing".to_string(),
                group: None,
                kind: None,
                namespace: None,
            },
            negate: false,
            resolved_values: None,
            descriptor: Box::from(""),
        },
    )
}

#[test]
fn mixed_any_of_skips_unresolved_direct_and_attributes_named_fallback() {
    let set = ConditionSet::AnyOf {
        any_of: vec![
            AnyOfClause::Condition(unresolved_secret_condition()),
            AnyOfClause::AllOf(AllOfGroup {
                name: Some("header-fallback".to_string()),
                all_of: vec![key_match("X-Fallback", &["yes"])],
            }),
        ],
    };
    let compiled = CompiledConditionSet::new(&set, "conditions", None);
    let session = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-Fallback".to_string(),
        },
        Some("yes".to_string()),
    )]);
    let mut log = PluginLog::new("test");

    let eval = compiled.evaluate(&session, Some(&mut log), true);
    assert_eq!(eval.state, MatchState::True);
    assert_eq!(eval.cause, Some("header-fallback"));
    assert!(log.contains("header-fallback"));
    assert!(log.contains("conditions.anyOf[1].allOf[0]:keyMatch:hdr:X-Fallback"));
}

#[test]
fn mixed_any_of_matching_direct_short_circuits_later_named_group() {
    let set = ConditionSet::AnyOf {
        any_of: vec![
            AnyOfClause::Condition(key_match("X-First", &["yes"])),
            AnyOfClause::AllOf(AllOfGroup {
                name: Some("must-not-run".to_string()),
                all_of: vec![key_match("X-Later", &["yes"])],
            }),
        ],
    };
    let compiled = CompiledConditionSet::new(&set, "conditions", None);
    let session = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-First".to_string(),
        },
        Some("yes".to_string()),
    )]);
    let mut log = PluginLog::new("test");

    let eval = compiled.evaluate(&session, Some(&mut log), true);
    assert_eq!(eval.state, MatchState::True);
    assert_eq!(eval.cause, Some("conditions.anyOf[0]:keyMatch:hdr:X-First"));
    assert!(!log.contains("must-not-run"));
}

#[tokio::test]
async fn unresolved_then_body_hmac_preserves_body_terminal_error() {
    let secret = b"body-hmac-secret-at-least-32-bytes";
    let date = Utc::now().to_rfc2822();
    let digest = "SHA-256=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let signing_string = format!("date: {date}\ndigest: {digest}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("valid HMAC key");
    mac.update(signing_string.as_bytes());
    let signature = STANDARD.encode(mac.finalize().into_bytes());
    let authorization =
        format!(r#"hmac username="alice", algorithm="hmac-sha256", headers="date digest", signature="{signature}""#);
    let body_hmac = Condition::HmacAuth(HmacMatchConfig {
        validate_request_body: true,
        resolved_credentials: Some(std::collections::HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: secret.to_vec(),
                headers: std::collections::HashMap::new(),
                group: "default".to_string(),
            },
        )])),
        ..Default::default()
    });
    let set = ConditionSet::all_of(vec![unresolved_secret_condition(), body_hmac]);
    let compiled = CompiledConditionSet::new(&set, "conditions", Some(BodyRequirement::fail_close()));
    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[
            ("authorization", authorization.as_str()),
            ("date", date.as_str()),
            ("digest", digest),
        ],
    );
    session
        .expect_ensure_request_body()
        .returning(|_| Err(BodyReadError::Timeout));
    session.expect_record_error().returning(|_| ());
    let mut log = PluginLog::new("test");

    assert!(matches!(
        compiled.evaluate_async(&mut session, &mut log, true).await,
        Err(PluginRunningResult::ErrResponse { status: 408, .. })
    ));
}

#[tokio::test]
async fn top_all_of_false_dominates_unresolved_without_reading_body_hmac() {
    let body_hmac = Condition::HmacAuth(HmacMatchConfig {
        validate_request_body: true,
        resolved_credentials: Some(std::collections::HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: b"body-hmac-secret-at-least-32-bytes".to_vec(),
                headers: std::collections::HashMap::new(),
                group: "default".to_string(),
            },
        )])),
        ..Default::default()
    });
    let set = ConditionSet::all_of(vec![
        unresolved_secret_condition(),
        key_match("X-False", &["yes"]),
        body_hmac,
    ]);
    let compiled = CompiledConditionSet::new(&set, "conditions", Some(BodyRequirement::fail_close()));
    let mut session = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-False".to_string(),
        },
        Some("no".to_string()),
    )]);
    let mut log = PluginLog::new("test");

    let eval = compiled
        .evaluate_async(&mut session, &mut log, true)
        .await
        .expect("false evaluation is non-terminal");
    assert_eq!(eval.state, MatchState::False);
    assert_eq!(eval.cause, Some("conditions.allOf[1]:keyMatch:hdr:X-False"));
}

fn core_values(values: Vec<&str>) -> MatchCore {
    MatchCore {
        values: Some(values.into_iter().map(str::to_string).collect()),
        ..MatchCore::default()
    }
}

fn core_regex(patterns: Vec<&str>) -> MatchCore {
    MatchCore {
        regex: Some(patterns.into_iter().map(str::to_string).collect()),
        ..MatchCore::default()
    }
}

#[test]
fn test_empty_conditions_run() {
    let conditions = PluginConditions::default();
    let mut mock = MockPluginSession::new();
    mock.expect_key_get().returning(|_| None);
    assert_eq!(conditions.evaluate(&mock), EvaluationResult::Run);
}

#[test]
fn test_skip_condition_matched() {
    let conditions = PluginConditions {
        skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
            key: KeyGet::Header {
                name: "X-Internal".to_string(),
            },
            negate: false,
            descriptor: Box::from(""),
        })])),
        run: None,
    };

    let session = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-Internal".to_string(),
        },
        Some("true".to_string()),
    )]);
    assert_eq!(conditions.evaluate(&session), EvaluationResult::Skip);
}

#[test]
fn test_skip_condition_not_matched() {
    let conditions = PluginConditions {
        skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
            key: KeyGet::Header {
                name: "X-Internal".to_string(),
            },
            negate: false,
            descriptor: Box::from(""),
        })])),
        run: None,
    };

    let mut mock = MockPluginSession::new();
    mock.expect_request_header_map().return_const(http::HeaderMap::new());
    assert_eq!(conditions.evaluate(&mock), EvaluationResult::Run);
}

#[test]
fn test_run_condition_all_satisfied() {
    let conditions = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![
            Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "Authorization".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Method,
                core: core_values(vec!["GET", "POST"]),
                negate: false,
                descriptor: Box::from(""),
            }),
        ])),
    };

    let session = create_mock_session_with_key_get(vec![
        (
            KeyGet::Header {
                name: "Authorization".to_string(),
            },
            Some("Bearer token".to_string()),
        ),
        (KeyGet::Method, Some("GET".to_string())),
    ]);
    assert_eq!(conditions.evaluate(&session), EvaluationResult::Run);
}

#[test]
fn test_run_condition_not_all_satisfied() {
    let conditions = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![
            Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "Authorization".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            }),
            Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Method,
                core: core_values(vec!["POST"]),
                negate: false,
                descriptor: Box::from(""),
            }),
        ])),
    };

    // Method is GET, not POST
    let session = create_mock_session_with_key_get(vec![
        (
            KeyGet::Header {
                name: "Authorization".to_string(),
            },
            Some("Bearer token".to_string()),
        ),
        (KeyGet::Method, Some("GET".to_string())),
    ]);
    assert_eq!(conditions.evaluate(&session), EvaluationResult::Skip);
}

#[test]
fn test_key_match_exact() {
    let condition = KeyMatchCondition {
        key: KeyGet::Header {
            name: "X-Environment".to_string(),
        },
        core: core_values(vec!["production"]),
        negate: false,
        descriptor: Box::from(""),
    };

    let session = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-Environment".to_string(),
        },
        Some("production".to_string()),
    )]);
    let mut log = PluginLog::new("test");
    assert!(condition.evaluate(&session, Some(&mut log)));

    let session2 = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-Environment".to_string(),
        },
        Some("staging".to_string()),
    )]);
    assert!(!condition.evaluate(&session2, Some(&mut log)));
}

#[test]
fn match_core_default_case_sensitive_and_unnegated_matches_today() {
    // YAML without caseSensitive / negate must keep today's verdicts:
    // exact match is case-sensitive; missing key is base false for KeyMatch.
    let key_match: KeyMatchCondition = serde_yaml::from_str(
        r#"
key:
  type: header
  name: X-Env
values:
  - production
"#,
    )
    .unwrap();
    assert!(key_match.core.case_sensitive);
    assert!(!key_match.negate);

    let mut log = PluginLog::new("test");
    let present = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-Env".to_string(),
        },
        Some("production".to_string()),
    )]);
    let case_mismatch = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-Env".to_string(),
        },
        Some("Production".to_string()),
    )]);
    let missing = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-Env".to_string(),
        },
        None,
    )]);

    assert!(key_match.evaluate(&present, Some(&mut log)));
    assert!(!key_match.evaluate(&case_mismatch, Some(&mut log)));
    assert!(!key_match.evaluate(&missing, Some(&mut log)));
}

#[test]
fn negated_key_match_missing_key_keeps_exclude_verdict() {
    // Former Exclude: missing key was true. KeyMatch base is false; negate
    // after the missing check yields true — Exclude-preserving.
    let condition = KeyMatchCondition {
        key: KeyGet::Header {
            name: "X-Env".to_string(),
        },
        core: core_values(vec!["production"]),
        negate: true,
        descriptor: Box::from(""),
    };
    let mut log = PluginLog::new("test");
    let missing = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-Env".to_string(),
        },
        None,
    )]);
    assert!(condition.evaluate(&missing, Some(&mut log)));
}

#[test]
fn negated_time_range_inverts_window() {
    let in_window = TimeRangeCondition {
        after: Some("2020-01-01T00:00:00Z".to_string()),
        before: Some("2030-12-31T23:59:59Z".to_string()),
        negate: true,
        descriptor: Box::from(""),
    };
    assert!(!in_window.evaluate());

    let past = TimeRangeCondition {
        after: Some("2020-01-01T00:00:00Z".to_string()),
        before: Some("2020-12-31T23:59:59Z".to_string()),
        negate: true,
        descriptor: Box::from(""),
    };
    assert!(past.evaluate());
}

#[test]
fn negated_remote_ip_match() {
    let condition = KeyMatchCondition {
        key: KeyGet::RemoteIp,
        core: core_values(vec!["1.2.3.4"]),
        negate: true,
        descriptor: Box::from(""),
    };
    let mut log = PluginLog::new("test");
    let hit = create_mock_session_with_key_get(vec![(KeyGet::RemoteIp, Some("1.2.3.4".to_string()))]);
    let miss = create_mock_session_with_key_get(vec![(KeyGet::RemoteIp, Some("5.6.7.8".to_string()))]);
    assert!(!condition.evaluate(&hit, Some(&mut log)));
    assert!(condition.evaluate(&miss, Some(&mut log)));
}

#[test]
fn test_time_range_within() {
    let condition = TimeRangeCondition {
        after: Some("2020-01-01T00:00:00Z".to_string()),
        before: Some("2030-12-31T23:59:59Z".to_string()),
        negate: false,
        descriptor: Box::from(""),
    };

    assert!(condition.evaluate());
}

#[test]
fn test_time_range_past() {
    let condition = TimeRangeCondition {
        after: Some("2020-01-01T00:00:00Z".to_string()),
        before: Some("2020-12-31T23:59:59Z".to_string()),
        negate: false,
        descriptor: Box::from(""),
    };

    assert!(!condition.evaluate());
}

#[test]
fn test_time_range_validate_rejects_malformed_after() {
    // Invalid month (13) — operator typo. Must surface as a preparse error
    // (caller pushes into preparse_errors → seal_fail_closed) instead of
    // being silently dropped at evaluation time, which would leave the
    // time-gate permanently inert. See F-002.
    let condition = TimeRangeCondition {
        after: Some("2024-13-01T00:00:00Z".to_string()),
        before: None,
        negate: false,
        descriptor: Box::from(""),
    };
    assert!(condition.validate().is_err());
}

#[test]
fn test_time_range_validate_rejects_malformed_before() {
    let condition = TimeRangeCondition {
        after: None,
        before: Some("not-a-timestamp".to_string()),
        negate: false,
        descriptor: Box::from(""),
    };
    assert!(condition.validate().is_err());
}

#[test]
fn test_time_range_validate_accepts_well_formed_bounds() {
    let condition = TimeRangeCondition {
        after: Some("2020-01-01T00:00:00Z".to_string()),
        before: Some("2030-12-31T23:59:59Z".to_string()),
        negate: false,
        descriptor: Box::from(""),
    };
    assert!(condition.validate().is_ok());
}

#[test]
fn test_time_range_validate_accepts_unset_bounds() {
    let condition = TimeRangeCondition {
        after: None,
        before: None,
        negate: false,
        descriptor: Box::from(""),
    };
    assert!(condition.validate().is_ok());
}

#[test]
fn test_probability_always() {
    let condition = ProbabilityCondition {
        ratio: 1.0,
        key: None,
        negate: false,
        descriptor: Box::from(""),
    };

    let mut mock = MockPluginSession::new();
    mock.expect_key_get().returning(|_| None);
    assert!(condition.evaluate(&mock));
}

#[test]
fn test_probability_never() {
    let condition = ProbabilityCondition {
        ratio: 0.0,
        key: None,
        negate: false,
        descriptor: Box::from(""),
    };

    let mut mock = MockPluginSession::new();
    mock.expect_key_get().returning(|_| None);
    assert!(!condition.evaluate(&mock));
}

#[test]
fn test_key_match_path_regex() {
    let mut condition = KeyMatchCondition {
        key: KeyGet::Path,
        core: core_regex(vec!["^/api/.*", "^/admin/.*"]),
        negate: false,
        descriptor: Box::from(""),
    };
    condition.compile().unwrap();

    let mut log = PluginLog::new("test");
    let session1 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/api/users".to_string()))]);
    assert!(condition.evaluate(&session1, Some(&mut log)));

    let session2 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/admin/settings".to_string()))]);
    assert!(condition.evaluate(&session2, Some(&mut log)));

    let session3 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/public/index.html".to_string()))]);
    assert!(!condition.evaluate(&session3, Some(&mut log)));
}

#[test]
fn test_negated_key_match_path_keeps_exclude_present_verdict() {
    let condition = KeyMatchCondition {
        key: KeyGet::Path,
        core: core_values(vec!["/health", "/ready", "/metrics"]),
        negate: true,
        descriptor: Box::from(""),
    };

    let mut log = PluginLog::new("test");
    let session1 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/api/users".to_string()))]);
    assert!(condition.evaluate(&session1, Some(&mut log)));

    let session2 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/health".to_string()))]);
    assert!(!condition.evaluate(&session2, Some(&mut log)));

    let session3 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/ready".to_string()))]);
    assert!(!condition.evaluate(&session3, Some(&mut log)));
}

#[test]
fn test_regex_prefix_equivalent_of_former_wildcard() {
    // Fixture mapping is prefix-only: `/api/*` → `^/api/.*`.
    let mut core = core_regex(vec!["^/api/.*"]);
    core.compile(false).unwrap();
    assert!(core.matches("/api/users"));
    assert!(!core.matches("/public/index"));
}

fn log_entries(log: &PluginLog) -> Vec<String> {
    log.log
        .as_ref()
        .map(|buf| buf.entries().map(str::to_string).collect())
        .unwrap_or_default()
}

fn key_match(name: &str, values: &[&str]) -> Condition {
    Condition::KeyMatch(KeyMatchCondition {
        key: KeyGet::Header { name: name.to_string() },
        core: core_values(values.to_vec()),
        negate: false,
        descriptor: Box::from(""),
    })
}

#[test]
fn descriptor_and_short_circuit_log_sequence() {
    // AND (run): first two match, third fails — only the two successes push.
    let mut run = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![
            key_match("X-A", &["yes"]),
            key_match("X-B", &["yes"]),
            key_match("X-C", &["yes"]),
        ])),
    };
    run.compile().expect("compile run descriptors");
    let run_session = create_mock_session_with_key_get(vec![
        (
            KeyGet::Header {
                name: "X-A".to_string(),
            },
            Some("yes".to_string()),
        ),
        (
            KeyGet::Header {
                name: "X-B".to_string(),
            },
            Some("yes".to_string()),
        ),
        (
            KeyGet::Header {
                name: "X-C".to_string(),
            },
            Some("no".to_string()),
        ),
    ]);
    let mut run_log = PluginLog::new("test");
    let run_eval = run.evaluate_detail(&run_session, &mut run_log);
    assert_eq!(run_eval.result, EvaluationResult::Skip);
    assert_eq!(
        log_entries(&run_log),
        vec![
            "run.allOf[0]:keyMatch:hdr:X-A".to_string(),
            "run.allOf[1]:keyMatch:hdr:X-B".to_string(),
        ]
    );

    // OR (skip): first match short-circuits — only that descriptor is pushed.
    let mut skip = PluginConditions {
        skip: Some(ConditionSet::AnyOf {
            any_of: vec![
                AnyOfClause::Condition(key_match("X-A", &["yes"])),
                AnyOfClause::Condition(key_match("X-B", &["yes"])),
            ],
        }),
        run: None,
    };
    skip.compile().expect("compile skip descriptors");
    let skip_session = create_mock_session_with_key_get(vec![
        (
            KeyGet::Header {
                name: "X-A".to_string(),
            },
            Some("yes".to_string()),
        ),
        (
            KeyGet::Header {
                name: "X-B".to_string(),
            },
            Some("yes".to_string()),
        ),
    ]);
    let mut skip_log = PluginLog::new("test");
    let skip_eval = skip.evaluate_detail(&skip_session, &mut skip_log);
    assert_eq!(skip_eval.result, EvaluationResult::Skip);
    assert_eq!(
        log_entries(&skip_log),
        vec!["skip.anyOf[0]:keyMatch:hdr:X-A".to_string()]
    );
}

#[test]
fn condition_log_stays_inside_tier1_or_records_spill() {
    let mut run = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![
            key_match("X-A", &["yes"]),
            key_match("X-B", &["yes"]),
            key_match("X-C", &["yes"]),
        ])),
    };
    run.compile().expect("compile descriptors");
    let session = create_mock_session_with_key_get(vec![
        (
            KeyGet::Header {
                name: "X-A".to_string(),
            },
            Some("yes".to_string()),
        ),
        (
            KeyGet::Header {
                name: "X-B".to_string(),
            },
            Some("yes".to_string()),
        ),
        (
            KeyGet::Header {
                name: "X-C".to_string(),
            },
            Some("yes".to_string()),
        ),
    ]);
    let mut log = PluginLog::new("host");
    let eval = run.evaluate_detail(&session, &mut log);
    assert_eq!(eval.result, EvaluationResult::Run);
    log.push("Allow");

    let entries = log_entries(&log);
    assert_eq!(
        entries,
        vec![
            "run.allOf[0]:keyMatch:hdr:X-A".to_string(),
            "run.allOf[1]:keyMatch:hdr:X-B".to_string(),
            "run.allOf[2]:keyMatch:hdr:X-C".to_string(),
            "Allow".to_string(),
        ]
    );
    let aggregate: usize = entries.iter().map(|e| e.len()).sum();
    assert!(
        aggregate <= crate::observe::logs::BUFFER_CAPACITY,
        "aggregate {aggregate} exceeded BUFFER_CAPACITY"
    );
    assert!(log.ulog.is_none(), "realistic N stays in tier-1");
    assert!(log.log_full.is_none());
}

#[test]
fn body_evaluate_stays_silent() {
    let mut skip = PluginConditions {
        skip: Some(ConditionSet::all_of(vec![key_match("X-A", &["yes"])])),
        run: None,
    };
    skip.compile().expect("compile descriptors");
    let session = create_mock_session_with_key_get(vec![(
        KeyGet::Header {
            name: "X-A".to_string(),
        },
        Some("yes".to_string()),
    )]);
    assert_eq!(skip.evaluate(&session), EvaluationResult::Skip);
}

fn live_ip_test_session(cidr: &str) -> MockPluginSession {
    use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
    use edgion_common::conf_sync::traits::ConfHandler;
    let store = EdgionConfigDataStore::new();
    let data = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"test-networks","namespace":"default"},
        "spec":{"data":{"type":"IpList","config":{"items":[{"name":"main","cidrs":[cidr]}]}}}
    }))
    .unwrap();
    store.full_set(&std::collections::HashMap::from([(
        "default/test-networks".into(),
        std::sync::Arc::new(data),
    )]));
    let view = std::sync::Arc::new(store.capture_snapshot());
    let mut session = MockPluginSession::new();
    let availability = view.clone();
    session
        .expect_ip_list_availability()
        .returning(move |key| availability.ip_list_availability(key));
    session
        .expect_ip_list_contains()
        .returning(move |key, ip| view.ip_list_contains(key, ip));
    session
}

#[test]
fn direct_peer_ip_match_in_run_is_reachable() {
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"policy","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"Mock","config":{"status":418},
          "conditions":{"run":{"allOf":[{"type":"directPeerIpMatch","ipRefs":[{"name":"test-networks"}]}]}}}]}
    }))
    .unwrap();
    let compiler = IpConditionCompiler::new(&owner).unwrap();
    let conditions = owner.spec.request_plugins.as_ref().unwrap()[0]
        .conditions
        .as_ref()
        .unwrap();
    let compiled = CompiledPluginConditions::with_ip(conditions, Some(&compiler));
    let mut hit = live_ip_test_session("10.0.0.0/8");
    hit.expect_direct_peer_ip().return_const("10.1.2.3".to_string());
    assert_eq!(compiled.evaluate(&hit), EvaluationResult::Run);
    let mut miss = live_ip_test_session("10.0.0.0/8");
    miss.expect_direct_peer_ip().return_const("203.0.113.1".to_string());
    assert_eq!(compiled.evaluate(&miss), EvaluationResult::Skip);
}

fn jwt_session(claims: Option<&'static str>) -> MockPluginSession {
    let mut s = MockPluginSession::new();
    s.expect_get_ctx_var().never();
    s.expect_get_ctx_var_ref()
        .returning(move |k| if k == "jwt_claims" { claims } else { None });
    s
}

fn jwt_match(claim: &str, values: &[&str], negate: bool) -> Condition {
    let mut c = JwtClaimMatchCondition {
        claim: claim.to_string(),
        core: core_values(values.to_vec()),
        negate,
        descriptor: Box::from(""),
    };
    c.compile().expect("jwtClaimMatch compiles");
    Condition::JwtClaimMatch(c)
}

#[test]
fn jwt_claim_match_present_absent_and_negated() {
    let mut log = PluginLog::new("test");
    let present = jwt_session(Some(r#"{"sub":"u1","roles":["admin","dev"]}"#));
    let absent = jwt_session(None);

    let hit = jwt_match("sub", &["u1"], false);
    assert!(hit.evaluate(&present, Some(&mut log)));
    assert!(!hit.evaluate(&absent, Some(&mut log)));

    let miss_value = jwt_match("sub", &["other"], false);
    assert!(!miss_value.evaluate(&present, Some(&mut log)));

    let nested = jwt_match("roles", &["admin"], false);
    assert!(nested.evaluate(&present, Some(&mut log)));

    let negated_present = jwt_match("sub", &["u1"], true);
    assert!(!negated_present.evaluate(&present, Some(&mut log)));
    assert!(negated_present.evaluate(&absent, Some(&mut log)));
}

fn cert_session(cert: Option<&'static crate::ctx::ClientCertInfo>) -> MockPluginSession {
    let mut s = MockPluginSession::new();
    s.expect_client_cert_info().return_const(cert);
    s
}

fn multi_san_cert() -> &'static crate::ctx::ClientCertInfo {
    static CERT: std::sync::OnceLock<crate::ctx::ClientCertInfo> = std::sync::OnceLock::new();
    CERT.get_or_init(|| crate::ctx::ClientCertInfo {
        subject: "CN=alice, O=acme".to_string(),
        sans: vec![
            "alice.example.com".to_string(),
            "uri:spiffe://edgion.io/ns/foo".to_string(),
            "email:alice@example.com".to_string(),
        ],
        cn: Some("alice".to_string()),
        fingerprint: "ab:cd".to_string(),
    })
}

fn mtls_match(
    field: edgion_resources::resources::edgion_plugins::conditions::MtlsField,
    san_type: Option<edgion_resources::resources::edgion_plugins::conditions::SanType>,
    values: &[&str],
    negate: bool,
) -> Condition {
    let mut c = MtlsMatchCondition {
        field,
        san_type,
        core: core_values(values.to_vec()),
        negate,
        descriptor: Box::from(""),
    };
    c.compile().expect("mtlsMatch compiles");
    Condition::MtlsMatch(c)
}

#[test]
fn mtls_match_present_absent_and_negated() {
    use edgion_resources::resources::edgion_plugins::conditions::MtlsField;
    let mut log = PluginLog::new("test");
    let present = cert_session(Some(multi_san_cert()));
    let absent = cert_session(None);

    let cn = mtls_match(MtlsField::Cn, None, &["alice"], false);
    assert!(cn.evaluate(&present, Some(&mut log)));
    assert!(!cn.evaluate(&absent, Some(&mut log)));

    let negated_cn = mtls_match(MtlsField::Cn, None, &["alice"], true);
    assert!(!negated_cn.evaluate(&present, Some(&mut log)));
    assert!(negated_cn.evaluate(&absent, Some(&mut log)));

    let subject = mtls_match(MtlsField::Subject, None, &["CN=alice, O=acme"], false);
    assert!(subject.evaluate(&present, Some(&mut log)));
}

#[test]
fn mtls_match_any_san_entry_on_multi_san_cert() {
    use edgion_resources::resources::edgion_plugins::conditions::{MtlsField, SanType};
    let mut log = PluginLog::new("test");
    let present = cert_session(Some(multi_san_cert()));

    let untyped = mtls_match(MtlsField::San, None, &["uri:spiffe://edgion.io/ns/foo"], false);
    assert!(untyped.evaluate(&present, Some(&mut log)));

    let dns = mtls_match(MtlsField::San, Some(SanType::Dns), &["alice.example.com"], false);
    assert!(dns.evaluate(&present, Some(&mut log)));

    let uri = mtls_match(
        MtlsField::San,
        Some(SanType::Uri),
        &["spiffe://edgion.io/ns/foo"],
        false,
    );
    assert!(uri.evaluate(&present, Some(&mut log)));

    let miss = mtls_match(MtlsField::San, None, &["other.example.com"], false);
    assert!(!miss.evaluate(&present, Some(&mut log)));
}

#[test]
fn request_path_differs_from_path_for_percent_encoded_admin() {
    let raw = "/%61dmin";
    let normalized = "/admin";
    let session = create_mock_session_with_key_get(vec![
        (KeyGet::Path, Some(normalized.to_string())),
        (KeyGet::RequestPath, Some(raw.to_string())),
    ]);
    assert_ne!(
        session.key_get(&KeyGet::Path),
        session.key_get(&KeyGet::RequestPath),
        "Path is normalized; RequestPath stays raw for /%61dmin"
    );
    assert_eq!(session.key_get(&KeyGet::RequestPath).as_deref(), Some(raw));
    assert_eq!(session.key_get(&KeyGet::Path).as_deref(), Some(normalized));

    let mut request_path_match = KeyMatchCondition {
        key: KeyGet::RequestPath,
        core: core_values(vec![raw]),
        negate: false,
        descriptor: Box::from(""),
    };
    request_path_match.compile().expect("requestPath keyMatch compiles");
    assert_eq!(request_path_match.core.values.as_ref().unwrap()[0], raw);

    let mut path_match = KeyMatchCondition {
        key: KeyGet::Path,
        core: core_values(vec![raw]),
        negate: false,
        descriptor: Box::from(""),
    };
    path_match.compile().expect("path keyMatch compiles");
    assert_eq!(path_match.core.values.as_ref().unwrap()[0], normalized);

    let mut log = PluginLog::new("test");
    assert!(request_path_match.evaluate(&session, Some(&mut log)));
    assert!(path_match.evaluate(&session, Some(&mut log)));
}

#[test]
fn unresolved_references_never_match_when_negated() {
    let session = MockPluginSession::new();
    let cases: Vec<Condition> = vec![
        serde_json::from_value(serde_json::json!({
            "type": "remoteIpMatch",
            "ipRefs": [{ "name": "missing" }],
            "negate": true
        }))
        .unwrap(),
        serde_json::from_value(serde_json::json!({
            "type": "secretMatch",
            "key": { "type": "header", "name": "X-Token" },
            "secretKey": "token",
            "secretRef": { "name": "missing" },
            "negate": true
        }))
        .unwrap(),
        serde_json::from_value(serde_json::json!({
            "type": "hmacAuth",
            "secretGroups": [{
                "name": "default",
                "secretRefs": [{ "name": "missing" }]
            }],
            "negate": true
        }))
        .unwrap(),
        serde_json::from_value(serde_json::json!({
            "type": "hmacAuth",
            "secretGroups": [{
                "name": "default",
                "secretRefs": [{ "name": "empty" }]
            }],
            "resolvedCredentials": {},
            "negate": true
        }))
        .unwrap(),
    ];

    for condition in cases {
        let set = ConditionSet::all_of(vec![condition]);
        let compiled = CompiledConditionSet::new(&set, "unbound", None);
        assert!(compiled.has_unresolved_refs());
        assert_eq!(compiled.evaluate(&session, None, false).state, MatchState::Unresolved);
    }
}

#[test]
fn resolved_empty_ip_union_is_not_unresolved() {
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"rap","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"RequestAccessPolicy","config":{
            "defaultProfile":"main","profiles":{},"resolvedProfiles":{"main":{"requiredRuleGroups":[{
                "name":"clients","anyOfRules":[{"name":"peer","type":"directPeerIp","config":{
                    "ipRefs":[{"name":"empty"}],"conditions":{"allOf":[{
                        "type":"remoteIpMatch","ipRefs":[{"name":"empty"}],"negate":true
                    }]}
                }}]
            }]}}
        }}]}
    }))
    .unwrap();
    let compiler = IpConditionCompiler::new(&owner).unwrap();
    let prepared =
        edgion_resources::resources::refs::condition_fields::PreparedConditionReferences::from_plugins(&owner);
    let session = live_ip_test_session("192.0.2.0/24");
    let mut predicates = Vec::new();
    prepared
        .visit_ip(&owner, |leaf| predicates.push(compiler.bind(leaf.config()).unwrap()))
        .unwrap();
    let ip = Some("192.0.2.1".parse().unwrap());
    assert_eq!(predicates[0].evaluate(&session, ip, false), MatchState::False);
    assert_eq!(predicates[1].evaluate(&session, ip, true), MatchState::True);
}

#[test]
fn invalid_ip_reference_payload_is_unresolved_even_with_inline_ips_and_negation() {
    for kind in ["directPeerIpMatch", "remoteIpMatch"] {
        let condition: Condition = serde_json::from_value(
            serde_json::json!({"type":kind,"ips":["0.0.0.0/0"],"ipRefs":[{"name":"bad/name"}],"negate":true}),
        )
        .unwrap();
        assert!(condition.has_unresolved_refs());
        assert!(!condition.evaluate(&MockPluginSession::new(), None));
    }
}

#[test]
fn mtls_regex_any_match_and_final_negation() {
    let session = cert_session(Some(multi_san_cert()));
    let mut condition = MtlsMatchCondition {
        field: MtlsField::San,
        san_type: Some(SanType::Uri),
        core: core_regex(vec!["^spiffe://edgion[.]io/"]),
        negate: false,
        descriptor: Box::from(""),
    };
    condition.compile().unwrap();
    assert!(evaluate_mtls_match(&condition, &session, None));
    condition.negate = true;
    assert!(!evaluate_mtls_match(&condition, &session, None));
    condition.san_type = Some(SanType::Email);
    assert!(evaluate_mtls_match(&condition, &session, None));
}

#[test]
fn mtls_missing_candidates_suppress_uncompiled_regex_diagnostic() {
    static CERT: std::sync::OnceLock<crate::ctx::ClientCertInfo> = std::sync::OnceLock::new();
    let cert = CERT.get_or_init(|| crate::ctx::ClientCertInfo {
        subject: "O=example".to_string(),
        cn: None,
        sans: vec!["IP:192.0.2.1".to_string()],
        fingerprint: "ab:cd".to_string(),
    });
    for (session, field, san_type) in [
        (cert_session(None), MtlsField::Subject, None),
        (cert_session(Some(cert)), MtlsField::Cn, None),
        (cert_session(Some(cert)), MtlsField::San, Some(SanType::Uri)),
    ] {
        for negate in [false, true] {
            let condition = MtlsMatchCondition {
                field,
                san_type,
                core: core_regex(vec!["example"]),
                negate,
                descriptor: Box::from(""),
            };
            let mut log = PluginLog::new("test");
            assert_eq!(evaluate_mtls_match(&condition, &session, Some(&mut log)), negate);
            assert!(log.log.is_none());
        }
    }
    let condition = MtlsMatchCondition {
        field: MtlsField::Subject,
        san_type: None,
        core: core_regex(vec!["missing"]),
        negate: false,
        descriptor: Box::from(""),
    };
    let mut log = PluginLog::new("test");
    assert!(!evaluate_mtls_match(
        &condition,
        &cert_session(Some(cert)),
        Some(&mut log)
    ));
    assert!(serde_json::to_string(&log).unwrap().contains("regex not compiled"));
}

impl CompiledConditionSet {
    pub(crate) fn new(set: &ConditionSet, prefix: &str, body_requirement: Option<BodyRequirement>) -> Self {
        Self::with_ip(set, prefix, body_requirement, None)
    }
}

#[test]
fn actual_owner_mixed_ip_and_uninitialized_regex_leaves_compile_at_every_consumer_root() {
    use edgion_resources::resources::refs::condition_fields::PreparedConditionReferences;
    use serde_json::json;
    let set = json!({"allOf":[
        {"type":"directPeerIpMatch","ipRefs":[{"name":"test-networks"}]},
        {"type":"keyMatch","key":{"type":"path"},"regex":["^/admin$"]}
    ]});
    let conditions = json!({"run":set});
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(json!({
        "metadata":{"name":"regex-roots","namespace":"default"},
        "spec":{"requestPlugins":[
            {"type":"Dsl","config":{"name":"regex-roots","source":"let found = req.body.contains(\"needle\")"},
             "conditions":conditions,"body":{"conditions":conditions},
             "dye":{"response":[{"name":"x-regex","on":["fail-close"],"conditions":conditions}]}},
            {"type":"RequestRestriction","config":{"mode":"Deny","conditions":set}},
            {"type":"RequestAccessPolicy","config":{"defaultProfile":"main","profiles":{},
             "resolvedProfiles":{"main":{"requiredRuleGroups":[{"name":"clients","anyOfRules":[
                 {"name":"peer","type":"directPeerIp","config":{"ipRefs":[{"name":"test-networks"}],"conditions":set}}
             ]}]}}}}
        ]}
    }))
    .unwrap();
    let compiler = IpConditionCompiler::new(&owner).unwrap();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut session = live_ip_test_session("10.0.0.0/8");
    session.expect_direct_peer_ip().return_const("10.1.2.3".into());
    session.expect_get_path().return_const("/admin".into());
    let mut ip_count = 0;
    let mut regex_count = 0;
    prepared
        .visit(&owner, |leaf| {
            let condition = leaf.config();
            let compiled = CompiledCondition::compile(condition, "root".into(), None, Some(&compiler));
            assert_eq!(compiled.evaluate_sync(&session), MatchState::True);
            match condition {
                Condition::DirectPeerIpMatch(_) => ip_count += 1,
                Condition::KeyMatch(_) => regex_count += 1,
                _ => unreachable!(),
            }
        })
        .unwrap();
    assert_eq!((ip_count, regex_count), (5, 5));
    let invalid: Condition =
        serde_json::from_value(json!({"type":"keyMatch","key":{"type":"path"},"regex":["["]})).unwrap();
    let invalid = CompiledCondition::compile(&invalid, "invalid".into(), None, None);
    assert!(invalid.unresolved);
    assert_eq!(invalid.evaluate_sync(&session), MatchState::Unresolved);
}

#[test]
fn borrowed_query_conditions_keep_present_empty_and_missing_distinct() {
    let mut session = MockPluginSession::new();
    session.expect_get_query_param_ref().returning(|name| match name {
        "present" => Some("hello world"),
        "empty" => Some(""),
        _ => None,
    });
    session.expect_key_get().never();
    for (name, expected) in [("present", true), ("empty", true), ("missing", false)] {
        let condition = KeyExistCondition {
            key: KeyGet::Query { name: name.to_string() },
            negate: false,
            descriptor: Box::from(""),
        };
        assert_eq!(condition.evaluate(&session), expected);
    }
    let condition = KeyMatchCondition {
        key: KeyGet::Query {
            name: "present".to_string(),
        },
        core: core_values(vec!["hello world"]),
        negate: false,
        descriptor: Box::from(""),
    };
    assert!(condition.evaluate(&session, None));
}

#[test]
fn insensitive_hashed_condition_borrows_already_folded_query_values() {
    let mut values: Vec<String> = (0..16).map(|i| format!("value{i}")).collect();
    values.push("é".into());
    let conditions = CompiledPluginConditions::new(PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Query { name: "mode".into() },
            core: MatchCore {
                values: Some(values),
                case_sensitive: false,
                ..Default::default()
            },
            negate: false,
            descriptor: Box::from(""),
        })])),
    });
    for (input, expected) in [
        ("value1", EvaluationResult::Run),
        ("absent", EvaluationResult::Skip),
        ("é", EvaluationResult::Run),
        ("éx", EvaluationResult::Skip),
        ("", EvaluationResult::Skip),
        ("VALUE1", EvaluationResult::Run),
    ] {
        let mut session = MockPluginSession::new();
        session.expect_get_query_param_ref().returning(move |_| Some(input));
        session.expect_get_query_param().never();
        assert_eq!(conditions.evaluate(&session), expected);
        let (result, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            std::hint::black_box(conditions.evaluate(&session))
        });
        assert_eq!(result, expected);
        if !input.bytes().any(|byte| byte.is_ascii_uppercase()) {
            assert_eq!(allocations.calls, 0);
            assert_eq!(allocations.requested_bytes, 0);
        }
    }
}
