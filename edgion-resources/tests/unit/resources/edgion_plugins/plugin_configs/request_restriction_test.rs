use super::*;
use crate::common::KeyGet;
use crate::resources::common::SecretObjectReference;
use crate::resources::config_data_ref::EdgionConfigDataRef;
use crate::resources::edgion_plugins::conditions::{
    AllOfGroup, AnyOfClause, Condition, IpAccessMatch, JwtClaimMatchCondition, KeyExistCondition, KeyMatchCondition,
    MatchCore, MtlsMatchCondition,
};
use crate::resources::edgion_plugins::hmac_match::HmacMatchConfig;
use crate::resources::edgion_plugins::plugin_configs::HmacAuthSecretGroup;

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

fn key_match_regex(key: KeyGet, patterns: &[&str]) -> Condition {
    Condition::KeyMatch(KeyMatchCondition {
        key,
        core: core_regex(patterns),
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

fn group(all_of: Vec<Condition>) -> AnyOfClause {
    AnyOfClause::AllOf(AllOfGroup { name: None, all_of })
}

fn deny_cfg(groups: Vec<AnyOfClause>) -> RequestRestrictionConfig {
    RequestRestrictionConfig {
        mode: Some(RestrictionMode::Deny),
        conditions: ConditionSet::AnyOf { any_of: groups },
        ..Default::default()
    }
}

#[test]
fn test_default_config() {
    let config = RequestRestrictionConfig::default();
    assert_eq!(config.mode, None);
    assert!(config.conditions.is_empty());
    assert_eq!(config.status, 403);
    assert_eq!(config.message, None);
}

#[test]
fn test_empty_groups_validation() {
    let mut config = RequestRestrictionConfig {
        mode: Some(RestrictionMode::Deny),
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(err.contains("allOf"), "{err}");
}

#[test]
fn test_missing_mode_is_rejected() {
    let mut config = RequestRestrictionConfig {
        mode: None,
        conditions: ConditionSet::AnyOf {
            any_of: vec![group(vec![key_match(KeyGet::Path, &["/admin"])])],
        },
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(err.contains("mode must be specified"));

    config.mode = Some(RestrictionMode::Allow);
    assert!(config.validate().is_ok());
}

#[test]
fn status_uses_final_response_range() {
    for status in [100, 101, 199, 600] {
        let mut config = deny_cfg(vec![group(vec![key_match(KeyGet::Path, &["/admin"])])]);
        config.status = status;
        assert!(config.validate().is_err(), "{status}");
    }
    for status in [200, 599] {
        let mut config = deny_cfg(vec![group(vec![key_match(KeyGet::Path, &["/admin"])])]);
        config.status = status;
        assert!(config.validate().is_ok(), "{status}");
    }

    let schema = serde_json::to_value(schemars::schema_for!(RequestRestrictionConfig)).unwrap();
    let status = &schema["properties"]["status"];
    assert_eq!(status["minimum"].as_u64(), Some(200));
    assert_eq!(status["maximum"].as_u64(), Some(599));
}

#[test]
fn test_allow_mode_resolution() {
    let mut c = RequestRestrictionConfig {
        mode: Some(RestrictionMode::Deny),
        ..Default::default()
    };
    assert!(!c.allow_mode());
    c.mode = Some(RestrictionMode::Allow);
    assert!(c.allow_mode());
}

#[test]
fn test_empty_all_of_in_group_validation() {
    let mut config = deny_cfg(vec![group(vec![])]);
    let err = config.validate().unwrap_err();
    assert!(err.contains("allOf"), "{err}");
}

#[test]
fn test_missing_key_for_header() {
    let mut config = deny_cfg(vec![group(vec![key_match(
        KeyGet::Header { name: String::new() },
        &["bad"],
    )])]);
    let err = config.validate().unwrap_err();
    assert!(err.contains("key"), "{err}");
}

#[test]
fn test_path_no_key_required() {
    let mut config = deny_cfg(vec![group(vec![key_match(KeyGet::Path, &["/admin"])])]);
    assert!(config.validate().is_ok());
}

#[test]
fn test_key_match_needs_values_or_regex() {
    let mut bad = deny_cfg(vec![group(vec![Condition::KeyMatch(KeyMatchCondition {
        key: KeyGet::Header { name: "X".to_string() },
        core: MatchCore::default(),
        negate: false,
        descriptor: Box::from(""),
    })])]);
    assert!(bad.validate().is_err());

    let mut ok = deny_cfg(vec![group(vec![key_exist(
        KeyGet::Header { name: "X".to_string() },
        true,
    )])]);
    assert!(ok.validate().is_ok());
}

#[test]
fn all_match_variants_require_values_or_regex() {
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
            field: crate::resources::edgion_plugins::conditions::MtlsField::Cn,
            san_type: None,
            core: MatchCore::default(),
            negate: true,
            descriptor: Box::from(""),
        }),
    ];

    for condition in conditions {
        let mut config = deny_cfg(vec![group(vec![condition])]);
        let checked = config.check_fields();
        let error = config.validate().expect_err("empty match payload must be rejected");
        assert_eq!(checked.as_deref(), Some(error.as_str()));
        assert!(error.contains("requires values or regex"), "{error}");
    }
}

#[test]
fn empty_values_are_rejected_even_with_valid_regex() {
    let mut config = deny_cfg(vec![group(vec![Condition::KeyMatch(KeyMatchCondition {
        key: KeyGet::Path,
        core: MatchCore {
            values: Some(vec![]),
            regex: Some(vec!["^/admin$".to_string()]),
            ..MatchCore::default()
        },
        negate: false,
        descriptor: Box::from(""),
    })])]);

    let checked = config.check_fields();
    let error = config
        .validate()
        .expect_err("an explicitly empty values array must be rejected");
    assert_eq!(checked.as_deref(), Some(error.as_str()));
    assert!(error.contains("keyMatch.values cannot be empty"), "{error}");
}

#[test]
fn blank_jwt_claim_is_rejected_by_shared_validation() {
    let mut config = deny_cfg(vec![group(vec![Condition::JwtClaimMatch(JwtClaimMatchCondition {
        claim: " \n".to_string(),
        core: core_values(&["admin"]),
        negate: true,
        descriptor: Box::from(""),
    })])]);

    let checked = config.check_fields();
    let error = config.validate().expect_err("blank JWT claim must be rejected");
    assert_eq!(checked.as_deref(), Some(error.as_str()));
    assert!(error.contains("jwtClaimMatch.claim cannot be empty"), "{error}");
}

#[test]
fn test_invalid_regex() {
    let mut config = deny_cfg(vec![group(vec![key_match_regex(
        KeyGet::Header {
            name: "X-Test".to_string(),
        },
        &["[invalid"],
    )])]);
    let err = config.validate().unwrap_err();
    assert!(err.contains("Invalid") || err.contains("regex"), "{err}");
}

#[test]
fn test_empty_values_list() {
    let mut config = deny_cfg(vec![group(vec![Condition::KeyMatch(KeyMatchCondition {
        key: KeyGet::Header {
            name: "X-Test".to_string(),
        },
        core: MatchCore {
            values: Some(vec![]),
            ..MatchCore::default()
        },
        negate: false,
        descriptor: Box::from(""),
    })])]);
    let err = config.validate().unwrap_err();
    assert!(
        err.contains("cannot be empty") || err.contains("values or regex"),
        "{err}"
    );
}

#[test]
fn ip_refs_in_rr_group_accepted() {
    let mut config = deny_cfg(vec![group(vec![Condition::DirectPeerIpMatch(IpAccessMatch {
        ips: vec!["10.0.0.0/8".to_string()],
        ip_refs: vec![EdgionConfigDataRef {
            name: "office".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    })])]);
    assert!(
        config.validate().is_ok(),
        "ipRefs in a RequestRestriction group must be accepted: {:?}",
        config.validate().err()
    );
}

#[test]
fn inline_ips_and_path_in_one_group_accepted() {
    let mut config = deny_cfg(vec![group(vec![
        Condition::DirectPeerIpMatch(IpAccessMatch {
            ips: vec!["127.0.0.1".to_string()],
            ..Default::default()
        }),
        key_match(KeyGet::Path, &["/admin"]),
    ])]);
    assert!(config.validate().is_ok());
}

#[test]
fn hmac_validate_request_body_accepted_in_rr_all_of() {
    let mut config = deny_cfg(vec![group(vec![Condition::HmacAuth(HmacMatchConfig {
        validate_request_body: true,
        secret_groups: Some(vec![HmacAuthSecretGroup {
            name: "default".to_string(),
            description: None,
            secret_refs: vec![SecretObjectReference {
                name: "hmac-credentials".to_string(),
                group: None,
                kind: None,
                namespace: None,
            }],
        }]),
        ..Default::default()
    })])]);
    assert!(
        config.validate().is_ok(),
        "HMAC validateRequestBody must stay accepted in RR allOf: {:?}",
        config.validate().err()
    );
    assert!(config.validates_request_body());
}

#[test]
fn check_fields_matches_validate_and_compile() {
    let mut invalid = deny_cfg(vec![group(vec![])]);
    assert_eq!(invalid.check_fields(), invalid.validate().err());

    let mut valid = deny_cfg(vec![group(vec![key_match(KeyGet::Path, &["/ok"])])]);
    assert_eq!(valid.check_fields(), valid.validate().err());
}

#[test]
fn jwt_claim_and_mtls_constructors_validate() {
    let mut jwt = deny_cfg(vec![group(vec![Condition::JwtClaimMatch(JwtClaimMatchCondition {
        claim: "realm_access.roles".to_string(),
        core: core_values(&["admin"]),
        negate: false,
        descriptor: Box::from(""),
    })])]);
    assert!(jwt.validate().is_ok());

    let mut mtls = deny_cfg(vec![group(vec![Condition::MtlsMatch(MtlsMatchCondition {
        field: crate::resources::edgion_plugins::conditions::MtlsField::Cn,
        san_type: None,
        core: core_values(&["alice"]),
        negate: false,
        descriptor: Box::from(""),
    })])]);
    assert!(mtls.validate().is_ok());
}
