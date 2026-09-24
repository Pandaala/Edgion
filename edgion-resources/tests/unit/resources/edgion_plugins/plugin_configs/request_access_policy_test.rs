use super::*;
use crate::common::KeyGet;
use crate::resources::common::SecretObjectReference;
use crate::resources::edgion_plugins::conditions::{
    Condition, JwtClaimMatchCondition, KeyMatchCondition, MatchCore, MtlsField, MtlsMatchCondition,
    SecretMatchCondition,
};
use crate::resources::edgion_plugins::plugin_configs::HmacMatchConfig;

fn ip_rule(name: &str, cidr: &str) -> RequestAccessRule {
    RequestAccessRule {
        name: name.to_string(),
        rule: RequestAccessRuleType::RemoteIp(IpAccessRuleConfig {
            ip: IpAccessMatch {
                ips: vec![cidr.to_string()],
                ..Default::default()
            },
            conditions: None,
        }),
    }
}

fn config() -> RequestAccessPolicyConfig {
    RequestAccessPolicyConfig {
        default_profile: "default".to_string(),
        profiles: HashMap::from([(
            "default".to_string(),
            RequestAccessProfile {
                required_rule_groups: vec![RequiredRuleGroup {
                    name: "client".to_string(),
                    any_of_rules: vec![ip_rule("internal", "10.0.0.0/8")],
                }],
            },
        )]),
        active_profile_ref: None,
        status: 403,
        message: None,
        description: None,
        resolved_profiles: None,
        resolution_warnings: vec![],
    }
}

#[test]
fn validates_required_group_model() {
    assert!(config().validate_and_init().is_ok());
}

#[test]
fn status_uses_final_response_range() {
    for status in [100, 101, 199, 600] {
        let mut value = config();
        value.status = status;
        assert!(value.validate_and_init().is_err(), "{status}");
    }
    for status in [200, 599] {
        let mut value = config();
        value.status = status;
        assert!(value.validate_and_init().is_ok(), "{status}");
    }

    let schema = serde_json::to_value(schemars::schema_for!(RequestAccessPolicyConfig)).unwrap();
    let status = &schema["properties"]["status"];
    assert_eq!(status["minimum"].as_u64(), Some(200));
    assert_eq!(status["maximum"].as_u64(), Some(599));
}

#[test]
fn all_of_rejects_empty_match_payloads() {
    let conditions = [
        Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
            core: MatchCore::default(),
            negate: true,
            descriptor: Box::from(""),
        }),
        Condition::JwtClaimMatch(JwtClaimMatchCondition {
            claim: "sub".to_string(),
            core: MatchCore::default(),
            negate: true,
            descriptor: Box::from(""),
        }),
        Condition::MtlsMatch(MtlsMatchCondition {
            field: MtlsField::Cn,
            san_type: None,
            core: MatchCore::default(),
            negate: true,
            descriptor: Box::from(""),
        }),
    ];

    for condition in conditions {
        let mut value = config();
        let RequestAccessRuleType::RemoteIp(rule) =
            &mut value.profiles.get_mut("default").unwrap().required_rule_groups[0].any_of_rules[0].rule
        else {
            panic!("expected remote IP rule")
        };
        rule.conditions = Some(ConditionSet::all_of(vec![condition]));

        let checked = value.check_fields();
        let error = value
            .validate_and_init()
            .expect_err("empty allOf match payload must be rejected");
        assert_eq!(checked.as_deref(), Some(error.as_str()));
        assert!(error.contains("requires values or regex"), "{error}");
    }
}

#[test]
fn all_of_rejects_blank_jwt_claim() {
    let mut value = config();
    let RequestAccessRuleType::RemoteIp(rule) =
        &mut value.profiles.get_mut("default").unwrap().required_rule_groups[0].any_of_rules[0].rule
    else {
        panic!("expected remote IP rule")
    };
    rule.conditions = Some(ConditionSet::all_of(vec![Condition::JwtClaimMatch(
        JwtClaimMatchCondition {
            claim: "\t ".to_string(),
            core: MatchCore {
                values: Some(vec!["admin".to_string()]),
                ..MatchCore::default()
            },
            negate: false,
            descriptor: Box::from(""),
        },
    )]));

    let checked = value.check_fields();
    let error = value.validate_and_init().expect_err("blank JWT claim must be rejected");
    assert_eq!(checked.as_deref(), Some(error.as_str()));
    assert!(error.contains("jwtClaimMatch.claim cannot be empty"), "{error}");
}

#[test]
fn rejects_empty_any_of_rules() {
    let mut value = config();
    value.profiles.get_mut("default").unwrap().required_rule_groups[0]
        .any_of_rules
        .clear();
    assert!(value
        .validate_and_init()
        .unwrap_err()
        .contains("anyOfRules cannot be empty"));
}

#[test]
fn rejects_duplicate_rule_names_within_group() {
    let mut value = config();
    value.profiles.get_mut("default").unwrap().required_rule_groups[0]
        .any_of_rules
        .push(ip_rule("internal", "192.0.2.0/24"));
    assert!(value
        .validate_and_init()
        .unwrap_err()
        .contains("duplicate any-of rule name"));
}

#[test]
fn serde_uses_required_groups_and_typed_rules() {
    let yaml = serde_yaml::to_string(&config()).unwrap();
    assert!(yaml.contains("requiredRuleGroups:"), "{yaml}");
    assert!(yaml.contains("anyOfRules:"), "{yaml}");
    assert!(yaml.contains("type: remoteIp"), "{yaml}");
    assert!(!yaml.contains("sourceMatch:"), "{yaml}");
}

#[test]
fn hmac_match_condition_rejects_standalone_only_fields() {
    for field in [
        "anonymous: guest",
        "realm: edge",
        "authFailureDelayMs: 1",
        "hideCredentials: true",
        "upstreamHeaderFields: [X-Consumer]",
    ] {
        let yaml = format!("type: hmacAuth\n{field}\n");
        assert!(serde_yaml::from_str::<Condition>(&yaml).is_err(), "accepted: {field}");
    }
}

#[test]
fn hmac_match_condition_redacts_admin_output_and_round_trips_conf_sync() {
    let condition = Condition::HmacAuth(HmacMatchConfig {
        resolved_credentials: Some(HashMap::from([(
            "alice".to_string(),
            crate::resources::edgion_plugins::plugin_configs::HmacCredential {
                secret: b"policy-conf-sync-secret".to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        )])),
        ..Default::default()
    });

    let admin_json = {
        let _guard = crate::redact::AdminRedactGuard::new();
        serde_json::to_string(&condition).unwrap()
    };
    assert!(!admin_json.contains("policy-conf-sync-secret"), "{admin_json}");
    assert!(admin_json.contains("[redacted]"), "{admin_json}");

    let wire = serde_json::to_vec(&condition).unwrap();
    let decoded: Condition = serde_json::from_slice(&wire).unwrap();
    let Condition::HmacAuth(config) = decoded else {
        panic!("unexpected condition type")
    };
    assert_eq!(
        config
            .resolved_credentials
            .as_ref()
            .and_then(|credentials| credentials.get("alice"))
            .map(|credential| credential.secret.as_slice()),
        Some(b"policy-conf-sync-secret".as_slice())
    );
}

#[test]
fn expanded_ip_union_compiles_one_matcher_for_inline_and_refs() {
    let mut value = IpAccessMatch {
        ips: vec!["10.0.0.0/8".to_string()],
        ip_refs: vec![EdgionConfigDataRef {
            name: "shared-networks".to_string(),
            ..Default::default()
        }],
        resolved_ips: Some(vec!["10.0.0.0/8".to_string(), "203.0.113.0/24".to_string()]),
        ..Default::default()
    };
    value.validate_and_init().unwrap();
    let matcher = value.matcher.as_ref().expect("one compiled union matcher");
    assert!(matcher.contains(&"10.1.2.3".parse().unwrap()));
    assert!(matcher.contains(&"203.0.113.8".parse().unwrap()));
}

#[test]
fn all_invalid_resolved_ips_compile_to_empty_matcher() {
    let mut value = IpAccessMatch {
        ips: vec!["10.0.0.0/8".to_string()],
        ip_refs: vec![EdgionConfigDataRef {
            name: "junk-networks".to_string(),
            ..Default::default()
        }],
        resolved_ips: Some(vec!["not-a-cidr".to_string(), "999.999.999.999".to_string()]),
        ..Default::default()
    };
    value
        .validate_and_init()
        .expect("all-invalid resolvedIps must not fail RAP init");
    let matcher = value.matcher.as_ref().expect("empty matcher");
    assert!(!matcher.contains(&"10.1.2.3".parse().unwrap()));
    assert!(!matcher.contains(&"203.0.113.8".parse().unwrap()));
}

#[test]
fn resolved_secret_values_are_redacted_in_admin_mode() {
    let condition = SecretMatchCondition {
        key: KeyGet::Header {
            name: "X-Origin-Token".to_string(),
        },
        secret_key: "token".to_string(),
        secret_ref: SecretObjectReference {
            name: "origin-token".to_string(),
            group: None,
            kind: None,
            namespace: None,
        },
        negate: false,
        resolved_values: Some(vec!["do-not-expose".to_string()]),
        descriptor: Box::from(""),
    };
    let json = {
        let _guard = crate::redact::AdminRedactGuard::new();
        serde_json::to_string(&condition).unwrap()
    };
    assert!(!json.contains("do-not-expose"), "{json}");
    assert!(json.contains("[redacted]"), "{json}");
}

#[test]
fn url_ref_validation_limits_enforce_hard_bounds() {
    assert!(UrlAllowListRefValidation::default().validate().is_ok());
    for value in [
        UrlAllowListRefValidation {
            max_url_rules: 0,
            ..Default::default()
        },
        UrlAllowListRefValidation {
            max_url_rules: MAX_REFERENCED_URL_RULES + 1,
            ..Default::default()
        },
        UrlAllowListRefValidation {
            max_ip_rules: 0,
            ..Default::default()
        },
        UrlAllowListRefValidation {
            max_ip_rules: MAX_REFERENCED_IP_RULES + 1,
            ..Default::default()
        },
    ] {
        assert!(value.validate().is_err());
    }
}

#[test]
fn resolved_url_candidate_secrets_redact_for_admin_and_round_trip_for_conf_sync() {
    let mut value = config();
    let candidate = UrlAccessCandidate {
        name: "vendor".to_string(),
        hosts: vec!["callbacks.example.com".to_string()],
        paths: vec![AccessPathMatch {
            match_type: AccessPathMatchType::Exact,
            value: "/callback".to_string(),
        }],
        conditions: Some(ConditionSet::all_of(vec![Condition::SecretMatch(
            SecretMatchCondition {
                key: KeyGet::Header {
                    name: "X-Origin-Token".to_string(),
                },
                secret_key: "token".to_string(),
                secret_ref: SecretObjectReference {
                    name: "origin-token".to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                },
                negate: false,
                resolved_values: Some(vec!["nested-policy-secret".to_string()]),
                descriptor: Box::from(""),
            },
        )])),
    };
    value.resolved_profiles = Some(HashMap::from([(
        "default".to_string(),
        RequestAccessProfile {
            required_rule_groups: vec![RequiredRuleGroup {
                name: "url".to_string(),
                any_of_rules: vec![RequestAccessRule {
                    name: "vendor".to_string(),
                    rule: RequestAccessRuleType::Url(UrlAccessRuleConfig {
                        hosts: vec![],
                        paths: vec![],
                        conditions: None,
                        config_refs: vec![RequestAccessUrlAllowListRef {
                            reference: EdgionConfigDataRef {
                                name: "vendor-list".to_string(),
                                ..Default::default()
                            },
                            validate: Default::default(),
                        }],
                        resolved_candidates: Some(vec![candidate]),
                    }),
                }],
            }],
        },
    )]));

    let admin_json = {
        let _guard = crate::redact::AdminRedactGuard::new();
        serde_json::to_string(&value).unwrap()
    };
    assert!(!admin_json.contains("nested-policy-secret"), "{admin_json}");
    assert!(admin_json.contains("[redacted]"), "{admin_json}");

    let wire = serde_json::to_vec(&value).unwrap();
    let decoded: RequestAccessPolicyConfig = serde_json::from_slice(&wire).unwrap();
    let RequestAccessRuleType::Url(url) =
        &decoded.runtime_profiles()["default"].required_rule_groups[0].any_of_rules[0].rule
    else {
        panic!("expected URL rule")
    };
    let secret_condition = url.resolved_candidates.as_ref().unwrap()[0]
        .conditions
        .as_ref()
        .unwrap()
        .iter_conditions()
        .next()
        .unwrap();
    let Condition::SecretMatch(secret) = secret_condition else {
        panic!("expected Secret match")
    };
    assert_eq!(
        secret.resolved_values.as_deref(),
        Some(["nested-policy-secret".to_string()].as_slice())
    );
}

#[test]
fn runtime_resolution_fields_are_absent_from_schema() {
    let schema = serde_json::to_string(&schemars::schema_for!(RequestAccessPolicyConfig)).unwrap();
    for field in [
        "resolvedProfiles",
        "resolutionWarnings",
        "resolvedCandidates",
        "resolvedIps",
        "resolvedValues",
        "resolvedCredentials",
    ] {
        assert!(
            !schema.contains(field),
            "schema exposed runtime field {field}: {schema}"
        );
    }
}

#[test]
fn validates_request_body_scans_all_of() {
    let mut value = config();
    value.profiles.get_mut("default").unwrap().required_rule_groups[0].any_of_rules[0].rule =
        RequestAccessRuleType::RemoteIp(IpAccessRuleConfig {
            ip: IpAccessMatch {
                ips: vec!["10.0.0.0/8".to_string()],
                ..Default::default()
            },
            conditions: Some(ConditionSet::all_of(vec![Condition::HmacAuth(HmacMatchConfig {
                validate_request_body: true,
                secret_groups: Some(vec![
                    crate::resources::edgion_plugins::plugin_configs::HmacAuthSecretGroup {
                        name: "default".to_string(),
                        description: None,
                        secret_refs: vec![SecretObjectReference {
                            name: "hmac-credentials".to_string(),
                            group: None,
                            kind: None,
                            namespace: None,
                        }],
                    },
                ]),
                ..Default::default()
            })])),
        });
    assert!(value.validates_request_body());
}
