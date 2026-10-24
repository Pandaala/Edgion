use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use crate::request_body::BodyReadError;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use edgion_resources::resources::edgion_plugins::conditions::{Condition, ConditionSet};
use edgion_resources::resources::edgion_plugins::plugin_configs::SecretMatchCondition;
use edgion_resources::resources::edgion_plugins::plugin_configs::{IpAccessMatch, UrlAccessCandidate};
use edgion_resources::EdgionErrorCode;
use hmac::{Hmac, Mac};
use sha2::Sha256;

const HMAC_SECRET: &[u8] = b"alice-super-secret-key";

fn parse_config(yaml: &str) -> RequestAccessPolicyConfig {
    serde_yaml::from_str(yaml).expect("valid policy fixture")
}

#[test]
fn plugin_name_is_static_across_instances() {
    let config = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: client
        anyOfRules:
          - name: internal
            type: remoteIp
            config: { ipRefs: [{ name: private-networks }] }
"#,
    );
    let first = RequestAccessPolicy::create(&config, "default", None);
    let second = RequestAccessPolicy::create(&config, "default", None);

    assert!(std::ptr::eq(first.name(), second.name()));
    assert_eq!(first.name(), "RequestAccessPolicy");
}

fn hmac_fallback_config(validate_request_body: bool) -> RequestAccessPolicyConfig {
    let mut config = parse_config(&format!(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: authenticated-or-internal
        anyOfRules:
          - name: signed
            type: directPeerIp
            config:
              ipRefs: [{{ name: peer-networks }}]
              conditions:
                allOf:
                  - type: hmacAuth
                    secretGroups:
                      - name: default
                        secretRefs: [{{ name: hmac-credentials }}]
                    validateRequestBody: {validate_request_body}
          - name: internal-fallback
            type: directPeerIp
            config: {{ ipRefs: [{{ name: peer-networks }}] }}
"#
    ));
    let rule = &mut config.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules[0];
    let RequestAccessRuleType::DirectPeerIp(rule) = &mut rule.rule else {
        panic!("unexpected rule type")
    };
    let Some(ConditionSet::AllOf { all_of }) = &mut rule.conditions else {
        panic!("expected supplemental conditions")
    };
    let Condition::HmacAuth(hmac) = &mut all_of[0] else {
        panic!("unexpected condition type")
    };
    hmac.resolved_credentials = Some(HashMap::from([(
        "alice".to_string(),
        edgion_resources::resources::edgion_plugins::HmacCredential {
            secret: HMAC_SECRET.to_vec(),
            headers: HashMap::new(),
            group: "default".to_string(),
        },
    )]));
    config
}

fn signed_authorization(date: &str, digest: Option<&str>) -> String {
    let (headers, signing_string) = match digest {
        Some(digest) => ("date digest", format!("date: {date}\ndigest: {digest}")),
        None => ("date", format!("date: {date}")),
    };
    let mut mac = Hmac::<Sha256>::new_from_slice(HMAC_SECRET).unwrap();
    mac.update(signing_string.as_bytes());
    let signature = STANDARD.encode(mac.finalize().into_bytes());
    format!(r#"hmac username="alice", algorithm="hmac-sha256", headers="{headers}", signature="{signature}""#)
}

#[tokio::test]
async fn unresolved_body_hmac_uses_backup_rule_without_negation() {
    let mut config = hmac_fallback_config(true);
    let rule = &mut config.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules[0];
    let RequestAccessRuleType::DirectPeerIp(rule) = &mut rule.rule else {
        unreachable!()
    };
    let Some(ConditionSet::AllOf { all_of }) = &mut rule.conditions else {
        unreachable!()
    };
    let Condition::HmacAuth(hmac) = &mut all_of[0] else {
        unreachable!()
    };
    hmac.resolved_credentials = None;
    hmac.negate = true;

    let plugin = RequestAccessPolicy::create(&config, "default", Some(BodyRequirement::fail_close()));
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("198.51.100.10".to_string());
    let mut log = PluginLog::new("RequestAccessPolicy");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn required_groups_are_and_and_inner_rules_are_or() {
    let config = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: peer
        anyOfRules:
          - name: internal-peer
            type: directPeerIp
            config: { ipRefs: [{ name: peer-networks }] }
      - name: client
        anyOfRules:
          - name: internal-client
            type: remoteIp
            config: { ipRefs: [{ name: private-networks }] }
          - name: vendor
            type: url
            config:
              hosts: [callbacks.example.com]
              paths: [{ type: PathPrefix, value: /callbacks }]
              conditions:
                allOf:
                  - type: remoteIpMatch
                    ipRefs: [{ name: client-networks }]
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("203.0.113.8".to_string());
    session.expect_get_path().return_const("/callbacks/acme".to_string());
    setup_header_mocks(&mut session, &[("host", "Callbacks.Example.Com:443")]);
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn one_failed_required_group_denies() {
    let config = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: peer
        anyOfRules:
          - name: internal-peer
            type: directPeerIp
            config: { ipRefs: [{ name: peer-networks }] }
      - name: client
        anyOfRules:
          - name: internal-client
            type: remoteIp
            config: { ipRefs: [{ name: private-networks }] }
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("203.0.113.8".to_string());
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert!(matches!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse { status: 403, .. }
    ));
}

#[tokio::test]
async fn parameterized_prefix_is_matched_by_the_profile_radix_tree() {
    let config = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: callback
        anyOfRules:
          - name: vendor
            type: url
            config:
              hosts: [callbacks.example.com]
              paths: [{ type: PathPrefix, value: /callbacks/:vendor }]
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("203.0.113.8".to_string());
    session
        .expect_get_path()
        .return_const("/callbacks/acme/events".to_string());
    setup_header_mocks(&mut session, &[("host", "callbacks.example.com")]);
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

fn imported_secret_config() -> RequestAccessPolicyConfig {
    let mut config = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: callback
        anyOfRules:
          - name: imported
            type: url
            config:
              configRefs:
                - name: vendor-list
"#,
    );
    let RequestAccessRuleType::Url(url) =
        &mut config.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules[0].rule
    else {
        panic!("expected URL rule")
    };
    url.resolved_candidates = Some(vec![UrlAccessCandidate {
        name: "vendor".to_string(),
        hosts: vec!["callbacks.example.com".to_string()],
        paths: vec![
            edgion_resources::resources::edgion_plugins::plugin_configs::AccessPathMatch {
                match_type: AccessPathMatchType::PathPrefix,
                value: "/callbacks/:vendor".to_string(),
            },
        ],
        conditions: Some(ConditionSet::all_of(vec![Condition::SecretMatch(
            SecretMatchCondition {
                key: edgion_resources::common::KeyGet::Header {
                    name: "X-Vendor-Token".to_string(),
                },
                secret_key: "token".to_string(),
                secret_ref: edgion_resources::resources::common::SecretObjectReference {
                    name: "vendor-token".to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                },
                negate: false,
                resolved_values: Some(vec!["expected-token".to_string()]),
                descriptor: Box::from(""),
            },
        )])),
    }]);
    config
}

async fn run_imported_secret_case(path: &str, host: &str, token: Option<&str>) -> PluginRunningResult {
    let plugin = RequestAccessPolicy::create(&imported_secret_config(), "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("203.0.113.8".to_string());
    session.expect_get_path().return_const(path.to_string());
    let mut headers = vec![("host", host)];
    headers.extend(token.map(|token| ("x-vendor-token", token)));
    setup_header_mocks(&mut session, &headers);
    let mut log = PluginLog::new("RequestAccessPolicy");
    plugin.run_request(&mut session, &mut log).await
}

#[tokio::test]
async fn imported_url_candidate_matches_path_host_and_nested_secret() {
    assert_eq!(
        run_imported_secret_case(
            "/callbacks/acme/events",
            "Callbacks.Example.Com:443",
            Some("expected-token")
        )
        .await,
        PluginRunningResult::GoodNext
    );
    for (path, host, token) in [
        ("/callbacks/acme/events", "callbacks.example.com", None),
        ("/callbacks/acme/events", "callbacks.example.com", Some("wrong-token")),
        ("/other/acme", "callbacks.example.com", Some("expected-token")),
        ("/callbacks/acme", "other.example.com", Some("expected-token")),
    ] {
        assert!(matches!(
            run_imported_secret_case(path, host, token).await,
            PluginRunningResult::ErrResponse { status: 403, .. }
        ));
    }
}

#[test]
fn profile_url_index_preserves_exact_prefix_regex_and_candidate_ownership() {
    use edgion_resources::resources::edgion_plugins::plugin_configs::AccessPathMatch;

    let candidates = [
        UrlAccessCandidate {
            name: "exact".to_string(),
            hosts: vec!["a.example.com".to_string()],
            paths: vec![AccessPathMatch {
                match_type: AccessPathMatchType::Exact,
                value: "/events/exact".to_string(),
            }],
            conditions: None,
        },
        UrlAccessCandidate {
            name: "prefix".to_string(),
            hosts: vec!["b.example.com".to_string()],
            paths: vec![AccessPathMatch {
                match_type: AccessPathMatchType::PathPrefix,
                value: "/events/:tenant".to_string(),
            }],
            conditions: None,
        },
        UrlAccessCandidate {
            name: "regex".to_string(),
            hosts: vec!["c.example.com".to_string()],
            paths: vec![AccessPathMatch {
                match_type: AccessPathMatchType::RegularExpression,
                value: r"^/events/v[0-9]+$".to_string(),
            }],
            conditions: None,
        },
        UrlAccessCandidate {
            name: "overlap".to_string(),
            hosts: vec![],
            paths: vec![
                AccessPathMatch {
                    match_type: AccessPathMatchType::Exact,
                    value: "/events/exact".to_string(),
                },
                AccessPathMatch {
                    match_type: AccessPathMatchType::PathPrefix,
                    value: "/events/:tenant".to_string(),
                },
                AccessPathMatch {
                    match_type: AccessPathMatchType::RegularExpression,
                    value: r"^/(events/(exact|v[0-9]+)|regex-first)$".to_string(),
                },
                AccessPathMatch {
                    match_type: AccessPathMatchType::RegularExpression,
                    value: r"^/regex-(first|later)$".to_string(),
                },
            ],
            conditions: None,
        },
    ];
    let index = RuntimeUrlIndex::compile(
        candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.hosts.as_slice(),
                    candidate.paths.as_slice(),
                    candidate.conditions.as_ref(),
                )
            })
            .collect(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(index.lookup("/events/exact"), vec![true, true, false, true]);
    assert_eq!(index.lookup("/events/exact/suffix"), vec![false, true, false, true]);
    assert_eq!(index.lookup("/events"), vec![false, false, false, false]);
    assert_eq!(index.lookup("/events/v12"), vec![false, true, true, true]);
    assert_eq!(index.lookup("/regex-first"), vec![false, false, false, true]);
    assert_eq!(index.lookup("/regex-later"), vec![false, false, false, true]);
    assert_eq!(index.lookup("/miss"), vec![false, false, false, false]);
}

#[tokio::test]
async fn empty_resolved_url_candidates_fail_closed_but_allow_sibling_rule() {
    let mut with_fallback = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: callback
        anyOfRules:
          - name: missing-list
            type: url
            config:
              configRefs: [{ name: missing-list }]
          - name: internal
            type: directPeerIp
            config: { ipRefs: [{ name: peer-networks }] }
"#,
    );
    let RequestAccessRuleType::Url(url) =
        &mut with_fallback.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules[0].rule
    else {
        panic!("expected URL rule")
    };
    url.resolved_candidates = Some(vec![]);
    let plugin = RequestAccessPolicy::create(&with_fallback, "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("203.0.113.8".to_string());
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );

    with_fallback.profiles.get_mut("lockdown").unwrap().required_rule_groups[0]
        .any_of_rules
        .truncate(1);
    let plugin = RequestAccessPolicy::create(&with_fallback, "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("203.0.113.8".to_string());
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert!(matches!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse { status: 403, .. }
    ));
}

#[tokio::test]
async fn imported_url_candidate_keeps_direct_and_remote_ip_distinct() {
    let mut config = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: callback
        anyOfRules:
          - name: imported
            type: url
            config:
              configRefs: [{ name: vendor-list }]
"#,
    );
    let RequestAccessRuleType::Url(url) =
        &mut config.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules[0].rule
    else {
        panic!("expected URL rule")
    };
    url.resolved_candidates = Some(vec![UrlAccessCandidate {
        name: "vendor".to_string(),
        hosts: vec!["callbacks.example.com".to_string()],
        paths: vec![
            edgion_resources::resources::edgion_plugins::plugin_configs::AccessPathMatch {
                match_type: AccessPathMatchType::Exact,
                value: "/callback".to_string(),
            },
        ],
        conditions: Some(ConditionSet::all_of(vec![
            Condition::DirectPeerIpMatch(IpAccessMatch {
                refs: edgion_resources::resources::edgion_plugins::ip_reference_set::IpReferenceSet {
                    ip_refs: vec![edgion_resources::resources::edgion_config_data::EdgionConfigDataRef {
                        name: "peer-networks".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            }),
            Condition::RemoteIpMatch(IpAccessMatch {
                refs: edgion_resources::resources::edgion_plugins::ip_reference_set::IpReferenceSet {
                    ip_refs: vec![edgion_resources::resources::edgion_config_data::EdgionConfigDataRef {
                        name: "client-networks".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            }),
        ])),
    }]);
    let run = |direct: &'static str, remote: &'static str, remote_reads: usize, config: RequestAccessPolicyConfig| async move {
        let plugin = RequestAccessPolicy::create(&config, "default", None);
        let mut session = ip_session();
        session
            .expect_direct_peer_ip()
            .times(1)
            .return_const(direct.to_string());
        session
            .expect_remote_ip()
            .times(remote_reads)
            .return_const(remote.to_string());
        session.expect_get_path().return_const("/callback".to_string());
        setup_header_mocks(&mut session, &[("host", "callbacks.example.com")]);
        let mut log = PluginLog::new("RequestAccessPolicy");
        plugin.run_request(&mut session, &mut log).await
    };
    assert_eq!(
        run("192.0.2.10", "203.0.113.8", 1, config.clone()).await,
        PluginRunningResult::GoodNext
    );
    assert!(matches!(
        run("203.0.113.8", "192.0.2.10", 0, config).await,
        PluginRunningResult::ErrResponse { status: 403, .. }
    ));
}

#[tokio::test]
async fn bad_hmac_is_a_non_match_and_allows_sibling_fallback() {
    let config = hmac_fallback_config(false);
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let date = chrono::Utc::now().to_rfc2822();
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("198.51.100.8".to_string());
    setup_header_mocks(
        &mut session,
        &[
            (
                "authorization",
                r#"hmac username="alice", algorithm="hmac-sha256", headers="date", signature="AAAA""#,
            ),
            ("date", date.as_str()),
        ],
    );
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn hmac_body_capture_failure_is_terminal_for_the_policy() {
    let config = hmac_fallback_config(true);
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let date = chrono::Utc::now().to_rfc2822();
    let digest = "SHA-256=unused".to_string();
    let authorization = signed_authorization(&date, Some(&digest));
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("198.51.100.8".to_string());
    setup_header_mocks(
        &mut session,
        &[
            ("authorization", authorization.as_str()),
            ("date", date.as_str()),
            ("digest", digest.as_str()),
        ],
    );
    session
        .expect_ensure_request_body()
        .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
    session.expect_record_error().returning(|_| ());
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

#[tokio::test]
async fn hmac_logical_body_limit_is_a_non_match_and_allows_sibling_fallback() {
    let config = hmac_fallback_config(true);
    let requirement = BodyRequirement {
        max_body_size: Some("4".to_string()),
        ..BodyRequirement::fail_close()
    };
    let plugin = RequestAccessPolicy::create(&config, "default", Some(requirement));
    let date = chrono::Utc::now().to_rfc2822();
    let digest = "SHA-256=unused".to_string();
    let authorization = signed_authorization(&date, Some(&digest));
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("198.51.100.8".to_string());
    setup_header_mocks(
        &mut session,
        &[
            ("authorization", authorization.as_str()),
            ("date", date.as_str()),
            ("digest", digest.as_str()),
        ],
    );
    session.expect_ensure_request_body().returning(|_| Ok(5));
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn hmac_digest_read_failure_is_terminal_for_the_policy() {
    let config = hmac_fallback_config(true);
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let date = chrono::Utc::now().to_rfc2822();
    let digest = "SHA-256=unused".to_string();
    let authorization = signed_authorization(&date, Some(&digest));
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("198.51.100.8".to_string());
    setup_header_mocks(
        &mut session,
        &[
            ("authorization", authorization.as_str()),
            ("date", date.as_str()),
            ("digest", digest.as_str()),
        ],
    );
    session.expect_ensure_request_body().returning(|_| Ok(1));
    session
        .expect_request_body_sha256()
        .returning(|| Err(BodyReadError::Timeout));
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::BodyReadTimeout)
        .times(1)
        .returning(|_| ());
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 408,
            body: None
        }
    );
}

#[tokio::test]
async fn time_range_in_all_of_is_reachable() {
    let config = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: client
        anyOfRules:
          - name: internal
            type: remoteIp
            config:
              ipRefs: [{ name: private-networks }]
              conditions:
                allOf:
                  - type: timeRange
                    after: "2000-01-01T00:00:00Z"
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("198.51.100.8".to_string());
    session.expect_remote_ip().return_const("10.1.2.3".to_string());
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn single_supplemental_leaf_suppresses_atomic_descriptor() {
    let config = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: peer
        anyOfRules:
          - name: internal
            type: directPeerIp
            config:
              ipRefs: [{ name: peer-networks }]
              conditions:
                allOf:
                  - type: keyExist
                    key: { type: header, name: X-One }
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("198.51.100.10".to_string());
    setup_header_mocks(&mut session, &[("x-one", "yes")]);
    let mut log = PluginLog::new("RequestAccessPolicy");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert!(log.contains("allow:matched"));
    assert!(!log.contains("conditions.allOf[0]:keyExist"));
}

#[tokio::test]
async fn multi_leaf_supplemental_set_retains_atomic_descriptors() {
    let config = parse_config(
        r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: peer
        anyOfRules:
          - name: internal
            type: directPeerIp
            config:
              ipRefs: [{ name: peer-networks }]
              conditions:
                allOf:
                  - type: keyExist
                    key: { type: header, name: X-One }
                  - type: keyExist
                    key: { type: header, name: X-Two }
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("198.51.100.10".to_string());
    setup_header_mocks(&mut session, &[("x-one", "yes"), ("x-two", "yes")]);
    let mut log = PluginLog::new("RequestAccessPolicy");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert!(log.contains("conditions.allOf[0]:keyExist:hdr:X-One"));
    assert!(log.contains("conditions.allOf[1]:keyExist:hdr:X-Two"));
}

#[tokio::test]
async fn partial_good_ip_payload_keeps_valid_sibling_matching() {
    let config: RequestAccessPolicyConfig = serde_json::from_value(serde_json::json!({"defaultProfile":"main","profiles":{"main":{"requiredRuleGroups":[{"name":"g","anyOfRules":[{"name":"peer","type":"directPeerIp","config":{"ipRefs":[{"name":"bad/name"},{"name":"nets"}]}}]}]}}})).unwrap();
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = ip_session();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("198.51.100.10".to_string());
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

// Build the same effective owning sidecars as Controller expansion before binding.
impl RequestAccessPolicy {
    pub(crate) fn create(
        config: &RequestAccessPolicyConfig,
        namespace: &str,
        body_requirement: Option<BodyRequirement>,
    ) -> Box<dyn RequestFilter> {
        let mut owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "fixture", "namespace": namespace},
            "spec": {"requestPlugins": [{"type": "RequestAccessPolicy", "config": config}]}
        }))
        .unwrap();
        let EdgionPlugin::RequestAccessPolicy(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin
        else {
            unreachable!()
        };
        if config.resolved_profiles.is_none() {
            config.resolved_profiles = Some(config.profiles.clone());
        }
        for profile in config.resolved_profiles.as_mut().unwrap().values_mut() {
            for group in &mut profile.required_rule_groups {
                for rule in &mut group.any_of_rules {
                    if let RequestAccessRuleType::Url(url) = &mut rule.rule {
                        if url.resolved_candidates.is_none() {
                            url.resolved_candidates = Some(vec![UrlAccessCandidate {
                                name: rule.name.clone(),
                                hosts: url.hosts.clone(),
                                paths: url.paths.clone(),
                                conditions: url.conditions.clone(),
                            }]);
                        }
                    }
                }
            }
        }
        Self::create_bound(
            HttpConfigDataEntry::from_plugins(&owner, 0).unwrap(),
            body_requirement,
            Some(&IpConditionCompiler::new(&owner).unwrap()),
        )
        .unwrap()
    }
}

#[tokio::test]
async fn owning_selector_invalid_and_denied_slots_keep_rap_filter_denial() {
    for reference in [
        serde_json::json!({"name":"bad/name"}),
        serde_json::json!({"name":"selector","namespace":""}),
        serde_json::json!({"name":"selector","refDenied":{"targetNamespace":"default","targetName":"selector"}}),
    ] {
        let owner: edgion_resources::EdgionPlugins=serde_json::from_value(serde_json::json!({"metadata":{"name":"rap"},"spec":{"requestPlugins":[{
            "type":"RequestAccessPolicy","config":{"defaultProfile":"base","activeProfileRef":reference,"profiles":{"base":{"requiredRuleGroups":[{"name":"all","anyOfRules":[{"name":"ip","type":"remoteIp","config":{"ipRefs":[{"name":"all-networks"}]}}]}]}}}
        }]}})).unwrap();
        let entry = HttpConfigDataEntry::from_plugins(&owner, 0).unwrap();
        let filter = RequestAccessPolicy::create_bound(entry, None, None).unwrap();
        let mut session = ip_session();
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert!(matches!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { .. }
        ));
        let EdgionPlugin::RequestAccessPolicy(config) = entry.plugin() else {
            unreachable!()
        };
        assert!(RequestAccessPolicy::create_inline(config, None, None).is_none());
        let mut invalid_owner = owner;
        invalid_owner.metadata.namespace = Some(String::new());
        assert!(RequestAccessPolicy::create_bound(
            HttpConfigDataEntry::from_plugins(&invalid_owner, 0).unwrap(),
            None,
            None
        )
        .is_none());
    }
}

/// Test sessions read real typed indexes; only the session wiring is mocked.
fn ip_session() -> MockPluginSession {
    use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
    use edgion_common::conf_sync::traits::ConfHandler;
    let store = EdgionConfigDataStore::new();
    let mut resources = HashMap::new();
    for (name, cidr) in [
        ("private-networks", "10.0.0.0/8"),
        ("peer-networks", "192.0.2.0/24"),
        ("client-networks", "203.0.113.0/24"),
        ("nets", "192.0.2.0/24"),
        ("all-networks", "0.0.0.0/0"),
    ] {
        let data = serde_json::from_value(serde_json::json!({"metadata":{"name":name,"namespace":"default"},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"main","cidrs":[cidr]}]}}}})).unwrap();
        resources.insert(format!("default/{name}"), Arc::new(data));
    }
    store.full_set(&resources);
    let view = Arc::new(store.capture_snapshot());
    let mut session = MockPluginSession::new();
    session.expect_ensure_config_data_view().return_const(());
    let availability = view.clone();
    session
        .expect_ip_list_availability()
        .returning(move |key| availability.ip_list_availability(key));
    session
        .expect_ip_list_contains()
        .returning(move |key, ip| view.ip_list_contains(key, ip));
    session
}

#[tokio::test]
async fn profile_reads_only_primary_ip_sources_once_before_url_fallback() {
    use serde_json::json;
    for (direct, remote) in [(false, false), (true, false), (false, true), (true, true)] {
        let url = json!({"name": "fallback", "type": "url", "config": {
            "paths": [{"type": "Exact", "value": "/allowed"}]
        }});
        let mut rules = Vec::new();
        for (needed, kind) in [(direct, "directPeerIp"), (remote, "remoteIp")] {
            if needed {
                for index in 0..2 {
                    rules.push(json!({"name": format!("{kind}-{index}"), "type": kind,
                        "config": {"ipRefs": [{"name": "unbound"}]}}));
                }
            }
        }
        rules.push(url.clone());
        let config: RequestAccessProfile = serde_json::from_value(json!({"requiredRuleGroups": [
            {"name": "url-first", "anyOfRules": [url]},
            {"name": "sources", "anyOfRules": rules}
        ]}))
        .unwrap();
        let profile = RuntimeProfile::new(&config, None, None).unwrap();
        for (direct_text, remote_text) in [("192.0.2.10", "2001:db8::1"), ("", "invalid")] {
            let mut session = MockPluginSession::new();
            session
                .expect_direct_peer_ip()
                .times(usize::from(direct))
                .return_const(direct_text.to_string());
            session
                .expect_remote_ip()
                .times(usize::from(remote))
                .return_const(remote_text.to_string());
            session.expect_get_path().times(1).return_const("/allowed".to_string());
            setup_header_mocks(&mut session, &[]);
            let mut log = PluginLog::new("RequestAccessPolicy");
            assert!(matches!(
                profile.allows(&mut session, &mut log).await,
                PolicyMatch::Matched
            ));
        }
    }
}

#[tokio::test]
async fn profile_host_demand_uses_imported_candidates_and_preserves_path_checks() {
    use serde_json::json;
    for (hosts, header, expected) in [
        (vec![], Some("Ignored.Example:443"), true),
        (vec![], None, true),
        (vec!["example.test"], Some("Example.Test.:443"), true),
        (vec!["example.test"], Some("other.test"), false),
        (vec!["example.test"], None, false),
        (vec![""], Some("other.test"), false),
        (vec![""], None, true),
    ] {
        let mut config: RequestAccessProfile = serde_json::from_value(json!({"requiredRuleGroups": [{
            "name": "urls", "anyOfRules": [{"name": "imported", "type": "url", "config": {
                "hosts": ["authored-decoy.test"], "paths": [{"type": "Exact", "value": "/decoy"}]
            }}]
        }]}))
        .unwrap();
        let RequestAccessRuleType::Url(url) = &mut config.required_rule_groups[0].any_of_rules[0].rule else {
            unreachable!()
        };
        url.resolved_candidates = Some(
            serde_json::from_value(json!([
                {"name": "hostless-first", "hosts": [], "paths": [{"type": "Exact", "value": "/other"}]},
                {"name": "selected", "hosts": hosts, "paths": [{"type": "Exact", "value": "/allowed"}]}
            ]))
            .unwrap(),
        );
        let profile = RuntimeProfile::new(&config, None, None).unwrap();
        for (path, allowed) in [("/allowed", expected), ("/missing", false)] {
            let mut session = MockPluginSession::new();
            session.expect_get_path().times(1).return_const(path.to_string());
            let mut headers = http::HeaderMap::new();
            if let Some(value) = header {
                headers.insert("host", http::HeaderValue::from_str(value).unwrap());
            }
            session
                .expect_request_header_map()
                .times(usize::from(!hosts.is_empty()))
                .return_const(headers);
            let mut log = PluginLog::new("RequestAccessPolicy");
            assert_eq!(
                matches!(profile.allows(&mut session, &mut log).await, PolicyMatch::Matched),
                allowed
            );
        }
    }
}

#[tokio::test]
async fn hostless_url_keeps_supplemental_host_condition_reads() {
    use edgion_resources::resources::edgion_plugins::conditions::KeyExistCondition;
    use serde_json::json;
    let mut config: RequestAccessProfile = serde_json::from_value(json!({"requiredRuleGroups": [{
        "name": "url", "anyOfRules": [{"name": "path", "type": "url", "config": {
            "paths": [{"type": "Exact", "value": "/allowed"}]
        }}]
    }]}))
    .unwrap();
    let RequestAccessRuleType::Url(url) = &mut config.required_rule_groups[0].any_of_rules[0].rule else {
        unreachable!()
    };
    url.conditions = Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
        key: edgion_resources::common::KeyGet::Header { name: "host".into() },
        negate: false,
        descriptor: Box::from(""),
    })]));
    let profile = RuntimeProfile::new(&config, None, None).unwrap();
    for present in [false, true] {
        let mut session = MockPluginSession::new();
        session.expect_get_path().times(1).return_const("/allowed".to_string());
        let mut headers = http::HeaderMap::new();
        if present {
            headers.insert("host", http::HeaderValue::from_static("example.test"));
        }
        session.expect_request_header_map().times(1).return_const(headers);
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            matches!(profile.allows(&mut session, &mut log).await, PolicyMatch::Matched),
            present
        );
    }
}

#[tokio::test]
async fn consecutive_url_rules_keep_imported_candidate_ownership_across_groups() {
    use serde_json::json;
    let mut config: RequestAccessProfile = serde_json::from_value(json!({"requiredRuleGroups": [
        {"name": "first", "anyOfRules": [
            {"name": "empty-import", "type": "url", "config": {"paths": [{"type": "Exact", "value": "/second"}]}},
            {"name": "multi-import", "type": "url", "config": {"paths": [{"type": "Exact", "value": "/second"}]}}
        ]},
        {"name": "second", "anyOfRules": [
            {"name": "common", "type": "url", "config": {"paths": [{"type": "Exact", "value": "/both"}]}},
            {"name": "second-only", "type": "url", "config": {"paths": [{"type": "Exact", "value": "/second"}]}}
        ]}
    ]}))
    .unwrap();
    for (rule, candidates) in config.required_rule_groups[0].any_of_rules.iter_mut().zip([
        json!([]),
        json!([
            {"name": "first-only", "paths": [{"type": "Exact", "value": "/first"}]},
            {"name": "common", "paths": [{"type": "Exact", "value": "/both"}]}
        ]),
    ]) {
        let RequestAccessRuleType::Url(url) = &mut rule.rule else {
            unreachable!()
        };
        url.resolved_candidates = Some(serde_json::from_value(candidates).unwrap());
    }
    let profile = RuntimeProfile::new(&config, None, None).unwrap();
    for (path, allowed) in [
        ("/both", true),
        ("/first", false),
        ("/second", false),
        ("/missing", false),
    ] {
        let mut session = MockPluginSession::new();
        session.expect_get_path().times(1).return_const(path.to_string());
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert_eq!(
            matches!(profile.allows(&mut session, &mut log).await, PolicyMatch::Matched),
            allowed
        );
    }
}

#[test]
fn profile_url_index_handles_more_than_eight_nested_prefixes() {
    use edgion_resources::resources::edgion_plugins::plugin_configs::AccessPathMatch;

    let paths: Vec<_> = (1..=12)
        .map(|depth| {
            vec![AccessPathMatch {
                match_type: AccessPathMatchType::PathPrefix,
                value: "/a".repeat(depth),
            }]
        })
        .collect();
    let index = RuntimeUrlIndex::compile(
        paths.iter().map(|paths| (&[][..], paths.as_slice(), None)).collect(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(index.lookup(&"/a".repeat(12)), vec![true; 12]);
    assert_eq!(index.lookup(&format!("{}/tail", "/a".repeat(12))), vec![true; 12]);
    for depth in 1..=12 {
        let exact: Vec<_> = (1..=12).map(|candidate| candidate <= depth).collect();
        let partial: Vec<_> = (1..=12).map(|candidate| candidate < depth).collect();
        assert_eq!(index.lookup(&"/a".repeat(depth)), exact);
        assert_eq!(index.lookup(&format!("{}x", "/a".repeat(depth))), partial);
    }
    assert_eq!(index.lookup("/missing"), vec![false; 12]);
}
