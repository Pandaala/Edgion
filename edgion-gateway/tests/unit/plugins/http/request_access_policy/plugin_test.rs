use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use crate::request_body::BodyReadError;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use edgion_resources::resources::edgion_plugins::conditions::{Condition, ConditionSet};
use edgion_resources::resources::edgion_plugins::plugin_configs::SecretMatchCondition;
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
            config: { ips: [10.0.0.0/8] }
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
              ips: [192.0.2.0/24]
              conditions:
                allOf:
                  - type: hmacAuth
                    secretGroups:
                      - name: default
                        secretRefs: [{{ name: hmac-credentials }}]
                    validateRequestBody: {validate_request_body}
          - name: internal-fallback
            type: directPeerIp
            config: {{ ips: [192.0.2.0/24] }}
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
    let mut session = MockPluginSession::new();
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
            config: { ips: [192.0.2.0/24] }
      - name: client
        anyOfRules:
          - name: internal-client
            type: remoteIp
            config: { ips: [10.0.0.0/8] }
          - name: vendor
            type: url
            config:
              hosts: [callbacks.example.com]
              paths: [{ type: PathPrefix, value: /callbacks }]
              conditions:
                allOf:
                  - type: remoteIpMatch
                    ips: [203.0.113.0/24]
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = MockPluginSession::new();
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
            config: { ips: [192.0.2.0/24] }
      - name: client
        anyOfRules:
          - name: internal-client
            type: remoteIp
            config: { ips: [10.0.0.0/8] }
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = MockPluginSession::new();
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
    let mut session = MockPluginSession::new();
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
    let mut session = MockPluginSession::new();
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

    let candidates = vec![
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
    ];
    let index = RuntimeUrlIndex::compile(candidates, None).unwrap();
    assert_eq!(index.lookup("/events/exact"), vec![true, true, false]);
    assert_eq!(index.lookup("/events/exact/suffix"), vec![false, true, false]);
    assert_eq!(index.lookup("/events"), vec![false, false, false]);
    assert_eq!(index.lookup("/events/v12"), vec![false, true, true]);
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
            config: { ips: [192.0.2.0/24] }
"#,
    );
    let RequestAccessRuleType::Url(url) =
        &mut with_fallback.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules[0].rule
    else {
        panic!("expected URL rule")
    };
    url.resolved_candidates = Some(vec![]);
    let plugin = RequestAccessPolicy::create(&with_fallback, "default", None);
    let mut session = MockPluginSession::new();
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
    let mut session = MockPluginSession::new();
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
                ips: vec!["192.0.2.0/24".to_string()],
                ..Default::default()
            }),
            Condition::RemoteIpMatch(IpAccessMatch {
                ips: vec!["203.0.113.0/24".to_string()],
                ..Default::default()
            }),
        ])),
    }]);
    let run = |direct: &'static str, remote: &'static str, config: RequestAccessPolicyConfig| async move {
        let plugin = RequestAccessPolicy::create(&config, "default", None);
        let mut session = MockPluginSession::new();
        session.expect_direct_peer_ip().return_const(direct.to_string());
        session.expect_remote_ip().return_const(remote.to_string());
        session.expect_get_path().return_const("/callback".to_string());
        setup_header_mocks(&mut session, &[("host", "callbacks.example.com")]);
        let mut log = PluginLog::new("RequestAccessPolicy");
        plugin.run_request(&mut session, &mut log).await
    };
    assert_eq!(
        run("192.0.2.10", "203.0.113.8", config.clone()).await,
        PluginRunningResult::GoodNext
    );
    assert!(matches!(
        run("203.0.113.8", "192.0.2.10", config).await,
        PluginRunningResult::ErrResponse { status: 403, .. }
    ));
}

#[tokio::test]
async fn bad_hmac_is_a_non_match_and_allows_sibling_fallback() {
    let config = hmac_fallback_config(false);
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let date = chrono::Utc::now().to_rfc2822();
    let mut session = MockPluginSession::new();
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
    let mut session = MockPluginSession::new();
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
    let mut session = MockPluginSession::new();
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
    let mut session = MockPluginSession::new();
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
              ips: [10.0.0.0/8]
              conditions:
                allOf:
                  - type: timeRange
                    after: "2000-01-01T00:00:00Z"
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = MockPluginSession::new();
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
              ips: [192.0.2.0/24]
              conditions:
                allOf:
                  - type: keyExist
                    key: { type: header, name: X-One }
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = MockPluginSession::new();
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
              ips: [192.0.2.0/24]
              conditions:
                allOf:
                  - type: keyExist
                    key: { type: header, name: X-One }
                  - type: keyExist
                    key: { type: header, name: X-Two }
"#,
    );
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = MockPluginSession::new();
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
    let config: RequestAccessPolicyConfig = serde_json::from_value(serde_json::json!({"defaultProfile":"main","profiles":{"main":{"requiredRuleGroups":[{"name":"g","anyOfRules":[{"name":"peer","type":"directPeerIp","config":{"ipRefs":[{"name":"bad/name"},{"name":"nets"}],"resolvedIps":["192.0.2.0/24"]}}]}]}}})).unwrap();
    let plugin = RequestAccessPolicy::create(&config, "default", None);
    let mut session = MockPluginSession::new();
    session.expect_direct_peer_ip().return_const("192.0.2.10".to_string());
    session.expect_remote_ip().return_const("198.51.100.10".to_string());
    let mut log = PluginLog::new("RequestAccessPolicy");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

// Detached fixture convenience is confined to this mirrored test module.
impl RequestAccessPolicy {
    pub(crate) fn create(
        config: &RequestAccessPolicyConfig,
        namespace: &str,
        body_requirement: Option<BodyRequirement>,
    ) -> Box<dyn RequestFilter> {
        let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "fixture", "namespace": namespace},
            "spec": {"requestPlugins": [{"type": "RequestAccessPolicy", "config": config}]}
        }))
        .unwrap();
        Self::create_bound(HttpConfigDataEntry::from_plugins(&owner, 0).unwrap(), body_requirement).unwrap()
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
            "type":"RequestAccessPolicy","config":{"defaultProfile":"base","activeProfileRef":reference,"profiles":{"base":{"requiredRuleGroups":[{"name":"all","anyOfRules":[{"name":"ip","type":"remoteIp","config":{"ips":["0.0.0.0/0"]}}]}]}}}
        }]}})).unwrap();
        let entry = HttpConfigDataEntry::from_plugins(&owner, 0).unwrap();
        let filter = RequestAccessPolicy::create_bound(entry, None).unwrap();
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("RequestAccessPolicy");
        assert!(matches!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { .. }
        ));
        let EdgionPlugin::RequestAccessPolicy(config) = entry.plugin() else {
            unreachable!()
        };
        assert!(RequestAccessPolicy::create_inline(config, None).is_none());
        let mut invalid_owner = owner;
        invalid_owner.metadata.namespace = Some(String::new());
        assert!(
            RequestAccessPolicy::create_bound(HttpConfigDataEntry::from_plugins(&invalid_owner, 0).unwrap(), None)
                .is_none()
        );
    }
}
