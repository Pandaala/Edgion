use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::{setup_condition_input_mocks, setup_header_mocks};
use edgion_resources::common::KeyGet;
use edgion_resources::resources::edgion_plugins::conditions::{
    AllOfGroup, AnyOfClause, Condition, ConditionSet, IpAccessMatch, KeyExistCondition, KeyMatchCondition, MatchCore,
};
use edgion_resources::resources::edgion_plugins::RestrictionMode;

fn core_values(values: &[&str]) -> MatchCore {
    MatchCore {
        values: Some(values.iter().map(|s| s.to_string()).collect()),
        ..MatchCore::default()
    }
}

fn core_regex(patterns: &[&str]) -> MatchCore {
    MatchCore {
        regex: Some(patterns.iter().map(|s| s.to_string()).collect()),
        ..MatchCore::default()
    }
}

fn key_match(key: KeyGet, values: &[&str]) -> Condition {
    Condition::KeyMatch(KeyMatchCondition {
        key,
        core: core_values(values),
        negate: false,
        descriptor: Box::from(""),
    })
}

fn key_exist(key: KeyGet, negate: bool) -> Condition {
    Condition::KeyExist(KeyExistCondition {
        key,
        negate,
        descriptor: Box::from(""),
    })
}

fn named_group(name: &str, all_of: Vec<Condition>) -> AnyOfClause {
    AnyOfClause::AllOf(AllOfGroup {
        name: Some(name.to_string()),
        all_of,
    })
}

fn cfg(mode: RestrictionMode, groups: Vec<AnyOfClause>) -> RequestRestrictionConfig {
    let mut c = RequestRestrictionConfig {
        mode: Some(mode),
        conditions: ConditionSet::AnyOf { any_of: groups },
        ..Default::default()
    };
    let _ = c.validate();
    c
}

fn expect_key(session: &mut MockPluginSession, want: KeyGet, value: Option<String>) {
    match value {
        Some(value) => setup_condition_input_mocks(session, &[(want, value.as_str())]),
        None => setup_condition_input_mocks(session, &[]),
    }
}

#[test]
fn plugin_name_is_static_across_generations() {
    let first = RequestRestriction::create(&cfg(
        RestrictionMode::Deny,
        vec![named_group("first", vec![key_match(KeyGet::Method, &["GET"])])],
    ));
    let second = RequestRestriction::create(&cfg(
        RestrictionMode::Allow,
        vec![named_group("second", vec![key_match(KeyGet::Method, &["POST"])])],
    ));

    assert_eq!(first.name(), PLUGIN_NAME);
    assert_eq!(second.name(), PLUGIN_NAME);
    assert!(std::ptr::eq(first.name().as_ptr(), second.name().as_ptr()));
}

#[tokio::test]
async fn matching_sync_all_of_logs_compiled_descriptors() {
    let config = cfg(
        RestrictionMode::Deny,
        vec![named_group(
            "audited",
            vec![
                key_match(KeyGet::Method, &["DELETE"]),
                key_exist(
                    KeyGet::Header {
                        name: "X-Audit-Token".to_string(),
                    },
                    false,
                ),
            ],
        )],
    );

    let plugin = RequestRestriction::create(&config);
    let mut session = MockPluginSession::new();
    setup_condition_input_mocks(
        &mut session,
        &[
            (KeyGet::Method, "DELETE"),
            (
                KeyGet::Header {
                    name: "X-Audit-Token".to_string(),
                },
                "present",
            ),
        ],
    );
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});
    let mut log = PluginLog::new("RequestRestriction");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("conditions.anyOf[0].allOf[0]:keyMatch:method"));
    assert!(log.contains("conditions.anyOf[0].allOf[1]:keyExist:hdr:X-Audit-Token"));
    assert!(log.contains("audited"));
}

#[tokio::test]
async fn unmatched_sync_predicate_does_not_log_descriptor() {
    let config = cfg(
        RestrictionMode::Deny,
        vec![named_group("readonly", vec![key_match(KeyGet::Method, &["GET"])])],
    );

    let plugin = RequestRestriction::create(&config);
    let mut session = MockPluginSession::new();
    expect_key(&mut session, KeyGet::Method, Some("POST".to_string()));
    let mut log = PluginLog::new("RequestRestriction");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert!(!log.contains("0:keyMatch:method"));
    assert!(log.contains("Allow"));
}

#[tokio::test]
async fn test_deny_mode_missing_header_match_denies() {
    // keyExist + negate: true => absent X-Auth-Token counts as a hit => deny.
    let config = cfg(
        RestrictionMode::Deny,
        vec![named_group(
            "missing-token",
            vec![key_exist(
                KeyGet::Header {
                    name: "X-Auth-Token".to_string(),
                },
                true,
            )],
        )],
    );
    let plugin = RequestRestriction::create(&config);
    let mut s = MockPluginSession::new();
    let mut log = PluginLog::new("RequestRestriction");
    expect_key(
        &mut s,
        KeyGet::Header {
            name: "X-Auth-Token".to_string(),
        },
        None,
    );
    s.expect_write_response_header().returning(|_, _| Ok(()));
    s.expect_write_response_body().returning(|_, _| Ok(()));
    s.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut s, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("Deny"));
}

#[tokio::test]
async fn test_request_path_source_uses_entry_raw() {
    // keyMatch + requestPath must read the entry-time raw path.
    let config = cfg(
        RestrictionMode::Deny,
        vec![named_group(
            "deny-raw-path",
            vec![key_match(KeyGet::RequestPath, &["/%61dmin"])],
        )],
    );
    let plugin = RequestRestriction::create(&config);
    let mut s = MockPluginSession::new();
    let mut log = PluginLog::new("RequestRestriction");
    expect_key(&mut s, KeyGet::RequestPath, Some("/%61dmin".to_string()));
    s.expect_write_response_header().returning(|_, _| Ok(()));
    s.expect_write_response_body().returning(|_, _| Ok(()));
    s.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut s, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("Deny"));
}

#[tokio::test]
async fn test_deny_mode_and_group_needs_all_predicates() {
    // Group = (Method == DELETE) AND (Path == /admin). DELETE /other must pass.
    let config = cfg(
        RestrictionMode::Deny,
        vec![named_group(
            "delete-admin",
            vec![
                key_match(KeyGet::Method, &["DELETE"]),
                key_match(KeyGet::Path, &["/admin"]),
            ],
        )],
    );
    let plugin = RequestRestriction::create(&config);
    let mut s = MockPluginSession::new();
    let mut log = PluginLog::new("RequestRestriction");
    setup_condition_input_mocks(&mut s, &[(KeyGet::Method, "DELETE"), (KeyGet::Path, "/other")]);

    let result = plugin.run_request(&mut s, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(log.contains("Allow"));
}

#[tokio::test]
async fn test_allow_mode_denies_when_no_group_matches() {
    // Allow mode: only GET passes. POST => deny.
    let config = cfg(
        RestrictionMode::Allow,
        vec![named_group("readonly", vec![key_match(KeyGet::Method, &["GET"])])],
    );
    let plugin = RequestRestriction::create(&config);
    let mut s = MockPluginSession::new();
    let mut log = PluginLog::new("RequestRestriction");
    expect_key(&mut s, KeyGet::Method, Some("POST".to_string()));
    s.expect_write_response_header().returning(|_, _| Ok(()));
    s.expect_write_response_body().returning(|_, _| Ok(()));
    s.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut s, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("Deny"));
}

#[tokio::test]
async fn custom_deny_body_is_prepared_once_and_survives_caller_changes() {
    use mockall::Sequence;
    use std::sync::Arc;

    use parking_lot::Mutex;

    const EXPECTED_BODY: &str = r#"{"message":"quoted \"slash\\line\n©"}"#;

    let mut config = cfg(
        RestrictionMode::Allow,
        vec![named_group("readonly", vec![key_match(KeyGet::Method, &["GET"])])],
    );
    config.status = 451;
    config.message = Some("quoted \"slash\\line\n©".to_string());
    let plugin = RequestRestriction::create(&config);

    config.status = 403;
    config.message = Some("caller generation changed".to_string());

    let observed = Arc::new(Mutex::new(Vec::new()));
    for _ in 0..2 {
        let mut session = MockPluginSession::new();
        expect_key(&mut session, KeyGet::Method, Some("POST".to_string()));
        let mut sequence = Sequence::new();
        session
            .expect_write_response_header()
            .withf(|response, end_of_stream| {
                !end_of_stream
                    && response.status.as_u16() == 451
                    && response
                        .headers
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        == Some("application/json")
            })
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        let seen = observed.clone();
        session
            .expect_write_response_body()
            .withf(|body, end_of_stream| *end_of_stream && body.as_deref() == Some(EXPECTED_BODY.as_bytes()))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |body, _| {
                let body = body.expect("deny response must carry the prepared body");
                seen.lock().push(body);
                Ok(())
            });
        session
            .expect_shutdown()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|| {});
        let mut log = PluginLog::new(PLUGIN_NAME);

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert_eq!(log.render_for_test(), "Deny (no allow alternative matched)");
    }

    let observed = observed.lock();
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0].as_ptr(), observed[1].as_ptr());
}

#[tokio::test]
async fn test_allow_mode_multi_group_or_passes_on_second_group() {
    // Allow-mode OR across groups: (Method GET) OR (X-Tier gold). A POST with
    // X-Tier: gold matches the second group => pass.
    let config = cfg(
        RestrictionMode::Allow,
        vec![
            named_group("readonly", vec![key_match(KeyGet::Method, &["GET"])]),
            named_group(
                "gold-tier",
                vec![key_match(
                    KeyGet::Header {
                        name: "X-Tier".to_string(),
                    },
                    &["gold"],
                )],
            ),
        ],
    );
    let plugin = RequestRestriction::create(&config);
    let mut s = MockPluginSession::new();
    let mut log = PluginLog::new("RequestRestriction");
    setup_condition_input_mocks(
        &mut s,
        &[
            (KeyGet::Method, "POST"),
            (
                KeyGet::Header {
                    name: "X-Tier".to_string(),
                },
                "gold",
            ),
        ],
    );

    let result = plugin.run_request(&mut s, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(log.contains("Allow"));
}

#[tokio::test]
async fn deny_mode_unresolved_reference_fails_closed() {
    let unresolved: Condition = serde_json::from_value(serde_json::json!({
        "type": "remoteIpMatch",
        "ipRefs": [{ "name": "missing" }],
        "negate": true
    }))
    .unwrap();
    let config = cfg(RestrictionMode::Deny, vec![named_group("blocked", vec![unresolved])]);
    let plugin = RequestRestriction::create(&config);
    let mut session = MockPluginSession::new();
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});
    let mut log = PluginLog::new("RequestRestriction");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("restriction config invalid"));
}

#[tokio::test]
async fn allow_mode_skips_unresolved_group_and_uses_backup_group() {
    let unresolved: Condition = serde_json::from_value(serde_json::json!({
        "type": "secretMatch",
        "key": { "type": "header", "name": "X-Token" },
        "secretKey": "token",
        "secretRef": { "name": "missing" },
        "negate": true
    }))
    .unwrap();
    let config = cfg(
        RestrictionMode::Allow,
        vec![
            named_group("incomplete", vec![unresolved]),
            named_group("backup", vec![key_match(KeyGet::Method, &["GET"])]),
        ],
    );
    let plugin = RequestRestriction::create(&config);
    let mut session = MockPluginSession::new();
    expect_key(&mut session, KeyGet::Method, Some("GET".to_string()));
    let mut log = PluginLog::new("RequestRestriction");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn allow_mode_skips_unresolved_body_hmac_group() {
    let unresolved: Condition = serde_json::from_value(serde_json::json!({
        "type": "hmacAuth",
        "secretGroups": [{
            "name": "default",
            "secretRefs": [{ "name": "missing" }]
        }],
        "validateRequestBody": true,
        "negate": true
    }))
    .unwrap();
    let config = cfg(
        RestrictionMode::Allow,
        vec![
            named_group("incomplete", vec![unresolved]),
            named_group("backup", vec![key_match(KeyGet::Method, &["GET"])]),
        ],
    );
    let plugin = RequestRestriction::create_with_body(&config, Some(BodyRequirement::fail_close()));
    let mut session = MockPluginSession::new();
    expect_key(&mut session, KeyGet::Method, Some("GET".to_string()));
    let mut log = PluginLog::new("RequestRestriction");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn test_invalid_config_fails_closed() {
    use mockall::Sequence;

    // An invalid regex fails validation at construction; run_request must fail
    // closed (terminate) rather than evaluate a half-compiled config.
    let config = cfg(
        RestrictionMode::Deny,
        vec![named_group(
            "bad",
            vec![Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Header { name: "X".to_string() },
                core: core_regex(&["[invalid"]),
                negate: false,
                descriptor: Box::from(""),
            })],
        )],
    );
    let plugin = RequestRestriction::create(&config);
    let mut s = MockPluginSession::new();
    let mut log = PluginLog::new("RequestRestriction");
    let mut sequence = Sequence::new();
    s.expect_write_response_header()
        .withf(|response, end_of_stream| {
            !end_of_stream
                && response.status.as_u16() == 500
                && response
                    .headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    == Some("application/json")
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    s.expect_write_response_body()
        .withf(|body, end_of_stream| *end_of_stream && body.as_deref() == Some(INVALID_POLICY_BODY))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    s.expect_shutdown().times(1).in_sequence(&mut sequence).returning(|| {});

    let result = plugin.run_request(&mut s, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert_eq!(log.render_for_test(), "err: restriction config invalid");
}

#[tokio::test]
async fn rule_groups_evaluated_in_config_order_first_match_short_circuits() {
    // Both groups would match GET. Configuration order and first-match
    // short-circuit pin the deny log to alpha; beta is never considered.
    let config = cfg(
        RestrictionMode::Deny,
        vec![
            named_group("alpha", vec![key_match(KeyGet::Method, &["GET"])]),
            named_group("beta", vec![key_match(KeyGet::Method, &["GET"])]),
        ],
    );
    let plugin = RequestRestriction::create(&config);
    let mut s = MockPluginSession::new();
    let mut log = PluginLog::new("RequestRestriction");
    expect_key(&mut s, KeyGet::Method, Some("GET".to_string()));
    s.expect_write_response_header().returning(|_, _| Ok(()));
    s.expect_write_response_body().returning(|_, _| Ok(()));
    s.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut s, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("Deny alpha"));
    assert!(!log.contains("beta"));
}

#[tokio::test]
async fn ip_and_path_in_one_group() {
    let config = cfg(
        RestrictionMode::Deny,
        vec![named_group(
            "ip-and-path",
            vec![
                Condition::DirectPeerIpMatch(IpAccessMatch {
                    refs: edgion_resources::resources::edgion_plugins::ip_reference_set::IpReferenceSet {
                        ip_refs: vec![edgion_resources::resources::edgion_config_data::EdgionConfigDataRef {
                            name: "test-networks".into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Path,
                    core: core_regex(&["^/admin$"]),
                    negate: false,
                    descriptor: Box::from(""),
                }),
            ],
        )],
    );
    use crate::plugins::runtime::conditions::ip_refs::IpConditionCompiler;
    use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
    use edgion_common::conf_sync::traits::ConfHandler;
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"restriction","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"RequestRestriction","config":config}]}
    }))
    .unwrap();
    let compiler = IpConditionCompiler::new(&owner).unwrap();
    let edgion_resources::resources::edgion_plugins::EdgionPlugin::RequestRestriction(config) =
        &owner.spec.request_plugins.as_ref().unwrap()[0].plugin
    else {
        unreachable!()
    };
    let plugin = RequestRestriction::with_ip(config, None, Some(&compiler));
    let store = EdgionConfigDataStore::new();
    let data = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"test-networks","namespace":"default"},
        "spec":{"data":{"type":"IpList","config":{"items":[{"name":"main","cidrs":["203.0.113.10/32"]}]}}}
    }))
    .unwrap();
    store.full_set(&std::collections::HashMap::from([(
        "default/test-networks".into(),
        std::sync::Arc::new(data),
    )]));
    let view = std::sync::Arc::new(store.capture_snapshot());
    let wire = |session: &mut MockPluginSession| {
        let availability = view.clone();
        session
            .expect_ip_list_availability()
            .returning(move |key| availability.ip_list_availability(key));
        let contains = view.clone();
        session
            .expect_ip_list_contains()
            .returning(move |key, ip| contains.ip_list_contains(key, ip));
    };

    let mut hit = MockPluginSession::new();
    wire(&mut hit);
    let mut hit_log = PluginLog::new("RequestRestriction");
    hit.expect_direct_peer_ip().return_const("203.0.113.10".to_string());
    setup_condition_input_mocks(&mut hit, &[(KeyGet::Path, "/admin")]);
    hit.expect_write_response_header().returning(|_, _| Ok(()));
    hit.expect_write_response_body().returning(|_, _| Ok(()));
    hit.expect_shutdown().returning(|| {});
    assert_eq!(
        plugin.run_request(&mut hit, &mut hit_log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(hit_log.contains("Deny"));

    let mut path_miss = MockPluginSession::new();
    wire(&mut path_miss);
    let mut path_log = PluginLog::new("RequestRestriction");
    path_miss
        .expect_direct_peer_ip()
        .return_const("203.0.113.10".to_string());
    setup_condition_input_mocks(&mut path_miss, &[(KeyGet::Path, "/public")]);
    assert_eq!(
        plugin.run_request(&mut path_miss, &mut path_log).await,
        PluginRunningResult::GoodNext
    );
    assert!(path_log.contains("Allow"));
}

/// Signed HMAC-body must match. The shared header-only evaluator returns
/// false whenever `validateRequestBody` is set; this test fails on that path.
#[tokio::test]
async fn hmac_body_all_of_matches_signed_request() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use edgion_resources::resources::edgion_plugins::hmac_match::HmacMatchConfig;
    use edgion_resources::resources::edgion_plugins::{HmacAlgorithm, HmacCredential};
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;

    const SECRET: &[u8] = b"alice-super-secret-key";
    let body = b"{\"n\":1}";
    let date = chrono::Utc::now().to_rfc2822();
    let digest = format!("SHA-256={}", STANDARD.encode(Sha256::digest(body)));
    let signing = format!("date: {date}\ndigest: {digest}");
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET).unwrap();
    mac.update(signing.as_bytes());
    let signature = STANDARD.encode(mac.finalize().into_bytes());
    let auth =
        format!(r#"hmac username="alice", algorithm="hmac-sha256", headers="date digest", signature="{signature}""#);

    let hmac = HmacMatchConfig {
        algorithms: vec![HmacAlgorithm::HmacSha256],
        clock_skew: 600,
        validate_request_body: true,
        resolved_credentials: Some(HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: SECRET.to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        )])),
        ..Default::default()
    };
    let config = cfg(
        RestrictionMode::Deny,
        vec![named_group("signed-body", vec![Condition::HmacAuth(hmac)])],
    );
    let plugin = RequestRestriction::create_with_body(&config, Some(BodyRequirement::fail_close()));
    drop(config);

    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[
            ("authorization", auth.as_str()),
            ("date", date.as_str()),
            ("digest", digest.as_str()),
        ],
    );
    session.expect_get_path().return_const("/v1/items".to_string());
    session.expect_get_query().returning(|| None);
    session.expect_get_method().return_const("POST".to_string());
    session
        .expect_request_body_sha256()
        .returning(move || Ok(Sha256::digest(body).into()));
    session
        .expect_ensure_request_body()
        .returning(|_| Ok(body.len() as u64));
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let mut log = PluginLog::new("RequestRestriction");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest, "{log:?}");
    assert!(
        !log.contains("hmacAuth"),
        "single-leaf match descriptor must be suppressed: {log:?}"
    );
    assert!(log.contains("Deny"), "{log:?}");
}

#[tokio::test]
async fn negated_hmac_body_no_match_logs_descriptor() {
    use edgion_resources::resources::edgion_plugins::hmac_match::HmacMatchConfig;
    use edgion_resources::resources::edgion_plugins::HmacCredential;
    use std::collections::HashMap;

    let hmac = HmacMatchConfig {
        validate_request_body: true,
        negate: true,
        resolved_credentials: Some(HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: b"alice-super-secret-key".to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        )])),
        ..Default::default()
    };
    let config = cfg(
        RestrictionMode::Deny,
        vec![named_group("unsigned-body", vec![Condition::HmacAuth(hmac)])],
    );
    let plugin = RequestRestriction::create_with_body(&config, Some(BodyRequirement::fail_close()));

    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[]);
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});
    let mut log = PluginLog::new("RequestRestriction");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest,
        "{log:?}"
    );
    assert!(
        !log.contains("hmacAuth"),
        "single-leaf match descriptor must be suppressed: {log:?}"
    );
    assert!(log.contains("Deny"), "{log:?}");
}

#[tokio::test]
async fn invalid_ip_identity_preserves_deny_failure_and_allow_fallback() {
    for mode in [RestrictionMode::Deny, RestrictionMode::Allow] {
        let condition: Condition = serde_json::from_value(
            serde_json::json!({"type":"remoteIpMatch","ipRefs":[{"name":"bad/name"}],"negate":true}),
        )
        .unwrap();
        let config = cfg(
            mode,
            vec![
                named_group("incomplete", vec![condition]),
                named_group("backup", vec![key_match(KeyGet::Method, &["GET"])]),
            ],
        );
        let plugin = RequestRestriction::create(&config);
        let mut session = MockPluginSession::new();
        if mode == RestrictionMode::Deny {
            session.expect_write_response_header().returning(|_, _| Ok(()));
            session.expect_write_response_body().returning(|_, _| Ok(()));
            session.expect_shutdown().returning(|| {});
        } else {
            expect_key(&mut session, KeyGet::Method, Some("GET".into()));
        }
        let mut log = PluginLog::new("RequestRestriction");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            if mode == RestrictionMode::Deny {
                PluginRunningResult::ErrTerminateRequest
            } else {
                PluginRunningResult::GoodNext
            }
        );
    }
}
