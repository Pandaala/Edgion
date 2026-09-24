use super::*;

#[test]
fn allow_mode_true_when_only_allow_refs_set() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let r = ConnectionIpRestrictionRules {
        allow_refs: Some(vec![EdgionConfigDataRef {
            name: "corp".into(),
            namespace: None,
            ..Default::default()
        }]),
        ..Default::default()
    };
    assert!(r.allow_mode());
}

#[test]
fn refs_only_config_passes_validation() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let mut r = ConnectionIpRestrictionRules {
        deny_refs: Some(vec![EdgionConfigDataRef {
            name: "blocklist".into(),
            namespace: None,
            ..Default::default()
        }]),
        ..Default::default()
    };
    assert!(r.validate_and_init().is_ok());
    assert_eq!(r.validation_error(), None);
}

#[test]
fn with_refs_deny_union_blocks_referenced_ip() {
    use crate::matcher::ip_cidr_helpers::build_grouped_matcher;
    let mut r = ConnectionIpRestrictionRules {
        deny: Some(vec![group("inline", &["1.1.1.1"])]),
        ..Default::default()
    };
    r.validate_and_init().unwrap();
    let ref_deny = build_grouped_matcher(&[group("reflist", &["2.2.2.2"])]).unwrap();
    assert!(matches!(
        r.check_ip_with_refs(&"2.2.2.2".parse().unwrap(), &[&ref_deny], &[]),
        AccessDecision::DenyByGroup(_)
    ));
    assert!(is_allowed(r.check_ip_with_refs(
        &"8.8.8.8".parse().unwrap(),
        &[&ref_deny],
        &[]
    )));
}

#[test]
fn allow_refs_missing_denies_all_in_allow_mode() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let r = ConnectionIpRestrictionRules {
        allow_refs: Some(vec![EdgionConfigDataRef {
            name: "missing".into(),
            namespace: None,
            ..Default::default()
        }]),
        ..Default::default()
    };
    assert!(matches!(
        r.check_ip_with_refs(&"8.8.8.8".parse().unwrap(), &[], &[]),
        AccessDecision::DenyByWhitelistMiss
    ));
}

fn group(name: &str, cidrs: &[&str]) -> IpGroup {
    IpGroup {
        name: name.to_string(),
        description: None,
        cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
    }
}

/// Maps an `AccessDecision` to a boolean allow/deny for test assertions.
fn is_allowed(decision: AccessDecision) -> bool {
    matches!(
        decision,
        AccessDecision::AllowByGroup(_) | AccessDecision::Default(DefaultAction::Allow)
    )
}

#[test]
fn must_have_allow_or_deny() {
    let mut r = ConnectionIpRestrictionRules::default();
    assert!(r.validate_and_init().is_err());
}

#[test]
fn allow_whitelist_ok() {
    let mut r = ConnectionIpRestrictionRules {
        allow: Some(vec![group("office", &["192.168.1.0/24"])]),
        ..Default::default()
    };
    r.validate_and_init().unwrap();
    assert!(is_allowed(r.check_ip(&"192.168.1.50".parse().unwrap())));
    assert!(matches!(
        r.check_ip(&"8.8.8.8".parse().unwrap()),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn deny_wins_over_allow() {
    let mut r = ConnectionIpRestrictionRules {
        allow: Some(vec![group("internal", &["10.0.0.0/8"])]),
        deny: Some(vec![group("blocked", &["10.0.0.100"])]),
        ..Default::default()
    };
    r.validate_and_init().unwrap();
    assert!(is_allowed(r.check_ip(&"10.0.0.1".parse().unwrap())));
    match r.check_ip(&"10.0.0.100".parse().unwrap()) {
        AccessDecision::DenyByGroup(g) => assert_eq!(&*g, "blocked"),
        other => panic!("expected DenyByGroup, got {:?}", other),
    }
}

#[test]
fn legacy_http_fields_captured_for_preparse() {
    // Old HTTP-schema fields must round-trip into `unknown_fields` so the Controller's
    // preparse step can emit a `LegacySchemaDeprecated` condition rather than failing
    // CR deserialization at the watcher level.
    let json = r#"{
            "allow": [{"name":"office","cidrs":["192.168.1.0/24"]}],
            "ipSource": "DirectPeerIp",
            "message": "blocked",
            "status": 403
        }"#;
    let rules: ConnectionIpRestrictionRules =
        serde_json::from_str(json).expect("legacy fields should now parse (detection moved to preparse)");
    let mut legacy = rules.legacy_http_fields();
    legacy.sort();
    assert_eq!(legacy, vec!["ipSource", "message", "status"]);
}

#[test]
fn clean_config_has_no_unknown_fields() {
    let json = r#"{
            "allow": [{"name":"office","cidrs":["192.168.1.0/24"]}],
            "defaultAction": "deny"
        }"#;
    let rules: ConnectionIpRestrictionRules = serde_json::from_str(json).unwrap();
    assert!(rules.legacy_http_fields().is_empty());
    assert!(rules.unknown_fields.is_empty());
}

#[test]
fn non_legacy_unknown_field_does_not_trigger_legacy_detection() {
    // A stray (non-legacy) field must not masquerade as a legacy-schema violation;
    // preparse only cares about the three HTTP fields from the old schema.
    let json = r#"{
            "allow": [{"name":"office","cidrs":["192.168.1.0/24"]}],
            "typoedField": "oops"
        }"#;
    let rules: ConnectionIpRestrictionRules = serde_json::from_str(json).unwrap();
    assert!(rules.legacy_http_fields().is_empty());
    assert!(rules.unknown_fields.contains_key("typoedField"));
}

#[test]
fn deny_only_default_deny_blocks_all_unmatched() {
    let mut r = ConnectionIpRestrictionRules {
        deny: Some(vec![group("blocked", &["1.2.3.4"])]),
        default_action: DefaultAction::Deny,
        ..Default::default()
    };
    r.validate_and_init().unwrap();
    // In deny list -> DenyByGroup.
    assert!(matches!(
        r.check_ip(&"1.2.3.4".parse().unwrap()),
        AccessDecision::DenyByGroup(_)
    ));
    // Not in deny, default Deny -> Default(Deny).
    assert!(matches!(
        r.check_ip(&"8.8.8.8".parse().unwrap()),
        AccessDecision::Default(DefaultAction::Deny)
    ));
}

#[test]
fn ipv6_allow_and_deny_work() {
    let mut r = ConnectionIpRestrictionRules {
        allow: Some(vec![group("v6-net", &["2001:db8::/32"])]),
        deny: Some(vec![group("v6-bad", &["2001:db8:dead::/48"])]),
        ..Default::default()
    };
    r.validate_and_init().unwrap();
    assert!(is_allowed(r.check_ip(&"2001:db8::1".parse().unwrap()))); // in allow
    assert!(matches!(
        r.check_ip(&"2001:db8:dead::1".parse().unwrap()),
        AccessDecision::DenyByGroup(_)
    )); // in deny (wins)
    assert!(matches!(
        r.check_ip(&"2001:db9::1".parse().unwrap()),
        AccessDecision::DenyByWhitelistMiss
    )); // outside allow
}

#[test]
fn whitelist_miss_returns_specific_variant() {
    let mut r = ConnectionIpRestrictionRules {
        allow: Some(vec![group("office", &["192.168.0.0/16"])]),
        ..Default::default()
    };
    r.validate_and_init().unwrap();
    let outside: IpAddr = "10.0.0.1".parse().unwrap();
    assert!(matches!(r.check_ip(&outside), AccessDecision::DenyByWhitelistMiss));
}

#[test]
fn validation_error_both_none_reports_missing_lists() {
    let rules = ConnectionIpRestrictionRules::default();
    assert_eq!(
        rules.validation_error(),
        Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string())
    );
}

#[test]
fn validation_error_is_stateless_and_keeps_matchers_unset() {
    let rules = ConnectionIpRestrictionRules {
        allow: Some(vec![group("office", &["192.168.1.0/24"])]),
        ..Default::default()
    };
    assert!(rules.validation_error().is_none());
    // Stateless: must not have built matchers as a side effect.
    assert!(rules.allow_matcher.is_none());
    assert!(rules.deny_matcher.is_none());
}

#[test]
fn validation_error_tolerates_invalid_cidr() {
    // Partial-good (CIDR-only leniency): a bad inline CIDR no longer rejects at
    // admission. Structural (non-CIDR) errors are still surfaced elsewhere.
    let rules = ConnectionIpRestrictionRules {
        allow: Some(vec![group("bad", &["not-an-ip"])]),
        ..Default::default()
    };
    assert!(rules.validation_error().is_none());
}

#[test]
fn deny_priority_returns_deny_group() {
    let mut r = ConnectionIpRestrictionRules {
        allow: Some(vec![group("allow_all", &["10.0.0.0/8"])]),
        deny: Some(vec![group("blocked", &["10.0.0.100/32"])]),
        ..Default::default()
    };
    r.validate_and_init().unwrap();
    let blocked: IpAddr = "10.0.0.100".parse().unwrap();
    match r.check_ip(&blocked) {
        AccessDecision::DenyByGroup(g) => assert_eq!(&*g, "blocked"),
        other => panic!("expected DenyByGroup, got {:?}", other),
    }
}
