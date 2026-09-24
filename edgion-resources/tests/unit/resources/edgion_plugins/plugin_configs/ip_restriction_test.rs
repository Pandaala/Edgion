use super::*;

fn group(name: &str, cidrs: &[&str]) -> IpGroup {
    IpGroup {
        name: name.to_string(),
        description: None,
        cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
    }
}

/// Convenience: `check_ip_access` returns an `AccessDecision`; the legacy
/// tests were boolean. This helper preserves their semantic intent.
fn is_allowed(decision: AccessDecision) -> bool {
    matches!(
        decision,
        AccessDecision::AllowByGroup(_) | AccessDecision::Default(DefaultAction::Allow)
    )
}

#[test]
fn test_default_config() {
    let config = IpRestrictionConfig::default();
    assert!(config.allow.is_none());
    assert!(config.deny.is_none());
    assert_eq!(config.ip_source, IpSource::RemoteIp);
    assert_eq!(config.message, None);
    assert_eq!(config.status, 403);
    assert_eq!(config.default_action, DefaultAction::Allow);
}

#[test]
fn test_missing_both_lists() {
    let mut config = IpRestrictionConfig::default();
    let result = config.validate_and_init();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("At least one"));
}

#[test]
fn test_invalid_status_code() {
    for status in [100, 101, 199, 600] {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![group("default", &["192.168.1.1"])]),
            status,
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err(), "{status}");
        assert!(result.unwrap_err().contains("Invalid status code"));
    }
}

#[test]
fn test_allow_list_validation() {
    let mut config = IpRestrictionConfig {
        allow: Some(vec![group("default", &["192.168.1.1", "10.0.0.0/8"])]),
        ..Default::default()
    };
    assert!(config.validate_and_init().is_ok());
}

#[test]
fn test_deny_list_validation() {
    let mut config = IpRestrictionConfig {
        deny: Some(vec![group("default", &["192.168.1.100", "172.16.0.0/12"])]),
        ..Default::default()
    };
    assert!(config.validate_and_init().is_ok());
}

#[test]
fn test_invalid_ip_is_partial_good() {
    // Partial-good (CIDR-only leniency): a bad inline IP is skipped, not fatal.
    let mut config = IpRestrictionConfig {
        allow: Some(vec![group("default", &["192.168.1.256"])]),
        ..Default::default()
    };
    let result = config.validate_and_init();
    assert!(result.is_ok());
    // All CIDRs invalid -> empty matcher (whitelist denies all), but still built.
    assert!(config.allow_matcher.is_some());
}

#[test]
fn test_invalid_cidr_prefix_is_partial_good() {
    // Partial-good (CIDR-only leniency): a bad CIDR prefix is skipped, not fatal.
    let mut config = IpRestrictionConfig {
        allow: Some(vec![group("default", &["192.168.1.0/33"])]),
        ..Default::default()
    };
    let result = config.validate_and_init();
    assert!(result.is_ok());
    assert!(config.allow_matcher.is_some());
}

#[test]
fn validate_and_init_partial_keeps_good_inline_cidr() {
    let mut cfg = IpRestrictionConfig {
        allow: Some(vec![IpGroup {
            name: "g".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into(), "bad-ip".into()],
        }]),
        ..Default::default()
    };
    // No longer an Err on a bad inline CIDR (partial-good); structural checks still pass.
    assert!(cfg.validate_and_init().is_ok());
    assert!(cfg.allow_matcher.is_some());
    // The good CIDR survived and matches.
    assert!(is_allowed(cfg.check_ip_access(&"10.1.2.3".parse().unwrap())));
}

#[test]
fn test_allow_list_access() {
    let mut config = IpRestrictionConfig {
        allow: Some(vec![group("default", &["192.168.1.0/24", "10.0.0.1"])]),
        ..Default::default()
    };
    config.validate_and_init().unwrap();

    assert!(is_allowed(config.check_ip_access(&"192.168.1.50".parse().unwrap())));
    assert!(is_allowed(config.check_ip_access(&"10.0.0.1".parse().unwrap())));

    // Not in allow list -> DenyByWhitelistMiss
    assert!(matches!(
        config.check_ip_access(&"8.8.8.8".parse().unwrap()),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn test_deny_list_access() {
    let mut config = IpRestrictionConfig {
        deny: Some(vec![group("default", &["192.168.1.100", "10.0.0.0/8"])]),
        ..Default::default()
    };
    config.validate_and_init().unwrap();

    assert!(matches!(
        config.check_ip_access(&"192.168.1.100".parse().unwrap()),
        AccessDecision::DenyByGroup(_)
    ));
    assert!(matches!(
        config.check_ip_access(&"10.0.0.1".parse().unwrap()),
        AccessDecision::DenyByGroup(_)
    ));

    // Not in deny list -> Default(Allow)
    assert!(is_allowed(config.check_ip_access(&"8.8.8.8".parse().unwrap())));
}

#[test]
fn test_allow_deny_combination() {
    let mut config = IpRestrictionConfig {
        allow: Some(vec![group("a", &["10.0.0.0/8"])]),
        deny: Some(vec![group("b", &["10.0.0.100"])]),
        ..Default::default()
    };
    config.validate_and_init().unwrap();

    assert!(is_allowed(config.check_ip_access(&"10.0.0.50".parse().unwrap())));
    assert!(matches!(
        config.check_ip_access(&"10.0.0.100".parse().unwrap()),
        AccessDecision::DenyByGroup(_)
    ));
    // Outside allow list -> DenyByWhitelistMiss
    assert!(matches!(
        config.check_ip_access(&"8.8.8.8".parse().unwrap()),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn test_default_action_deny() {
    let mut config = IpRestrictionConfig {
        deny: Some(vec![group("default", &["192.168.1.100"])]),
        default_action: DefaultAction::Deny,
        ..Default::default()
    };
    config.validate_and_init().unwrap();

    assert!(matches!(
        config.check_ip_access(&"192.168.1.100".parse().unwrap()),
        AccessDecision::DenyByGroup(_)
    ));
    // Not in deny list -> Default(Deny)
    assert!(matches!(
        config.check_ip_access(&"8.8.8.8".parse().unwrap()),
        AccessDecision::Default(DefaultAction::Deny)
    ));
}

#[test]
fn test_ipv6_support() {
    let mut config = IpRestrictionConfig {
        allow: Some(vec![group("default", &["::1", "2001:db8::/32"])]),
        ..Default::default()
    };
    config.validate_and_init().unwrap();

    assert!(is_allowed(config.check_ip_access(&"::1".parse().unwrap())));
    assert!(is_allowed(config.check_ip_access(&"2001:db8::1".parse().unwrap())));
    assert!(matches!(
        config.check_ip_access(&"2001:db9::1".parse().unwrap()),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn test_empty_allow_list() {
    let mut config = IpRestrictionConfig {
        allow: Some(vec![]),
        ..Default::default()
    };
    let result = config.validate_and_init();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("At least one"));
}

#[test]
fn test_serialization() {
    let config = IpRestrictionConfig {
        allow: Some(vec![group("a", &["192.168.1.0/24"])]),
        deny: Some(vec![group("b", &["192.168.1.100"])]),
        ip_source: IpSource::DirectPeerIp,
        message: Some("Access denied".to_string()),
        status: 404,
        default_action: DefaultAction::Deny,
        ..Default::default()
    };

    let json = serde_json::to_value(&config).unwrap();
    let deserialized: IpRestrictionConfig = serde_json::from_value(json).unwrap();

    assert_eq!(
        deserialized.allow.as_ref().map(|v| v.len()),
        config.allow.as_ref().map(|v| v.len())
    );
    assert_eq!(
        deserialized.deny.as_ref().map(|v| v.len()),
        config.deny.as_ref().map(|v| v.len())
    );
    assert_eq!(deserialized.allow.as_ref().unwrap()[0].name, "a");
    assert_eq!(deserialized.deny.as_ref().unwrap()[0].name, "b");
    assert_eq!(deserialized.ip_source, IpSource::DirectPeerIp);
    assert_eq!(deserialized.message, Some("Access denied".to_string()));
    assert_eq!(deserialized.status, 404);
    assert_eq!(deserialized.default_action, DefaultAction::Deny);
}

#[test]
fn ip_source_uses_exact_pascal_case_and_rejects_old_values() {
    assert_eq!(serde_json::to_string(&IpSource::RemoteIp).unwrap(), r#""RemoteIp""#);
    assert_eq!(
        serde_json::to_string(&IpSource::DirectPeerIp).unwrap(),
        r#""DirectPeerIp""#
    );
    for old in ["clientIp", "remoteAddr", "ClientIp", "RemoteAddr"] {
        assert!(
            serde_json::from_str::<IpSource>(&format!(r#""{old}""#)).is_err(),
            "{old} must be rejected"
        );
    }
}

#[test]
fn whitelist_miss_returns_specific_variant() {
    let mut config = IpRestrictionConfig {
        allow: Some(vec![IpGroup {
            name: "office".into(),
            description: None,
            cidrs: vec!["192.168.0.0/16".into()],
        }]),
        deny: None,
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    config.validate_and_init().unwrap();
    let outside: IpAddr = "10.0.0.1".parse().unwrap();
    assert!(matches!(
        config.check_ip_access(&outside),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn deny_priority_returns_deny_group() {
    let mut config = IpRestrictionConfig {
        allow: Some(vec![IpGroup {
            name: "allow_all".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }]),
        deny: Some(vec![IpGroup {
            name: "blocked".into(),
            description: None,
            cidrs: vec!["10.0.0.100/32".into()],
        }]),
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    config.validate_and_init().unwrap();
    let blocked: IpAddr = "10.0.0.100".parse().unwrap();
    match config.check_ip_access(&blocked) {
        AccessDecision::DenyByGroup(g) => assert_eq!(&*g, "blocked"),
        other => panic!("expected DenyByGroup, got {:?}", other),
    }
}

#[test]
fn allow_mode_true_when_only_allow_refs_set() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let config = IpRestrictionConfig {
        allow_refs: Some(vec![EdgionConfigDataRef {
            name: "corp".into(),
            namespace: None,
            ..Default::default()
        }]),
        ..Default::default()
    };
    assert!(config.allow_mode());
}

#[test]
fn refs_only_config_passes_validation() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let mut config = IpRestrictionConfig {
        deny_refs: Some(vec![EdgionConfigDataRef {
            name: "blocklist".into(),
            namespace: None,
            ..Default::default()
        }]),
        ..Default::default()
    };
    // No inline allow/deny, only denyRefs -> must be accepted now.
    assert!(config.validate_and_init().is_ok());
    assert_eq!(PluginValidation::validation_error(&config), None);
}

#[test]
fn with_refs_deny_union_blocks_referenced_ip() {
    use crate::matcher::ip_cidr_helpers::build_grouped_matcher;
    let mut config = IpRestrictionConfig {
        deny: Some(vec![group("inline", &["1.1.1.1"])]),
        ..Default::default()
    };
    config.validate_and_init().unwrap();
    let ref_deny = build_grouped_matcher(&[group("reflist", &["2.2.2.2"])]).unwrap();
    // referenced deny IP
    assert!(matches!(
        config.check_ip_access_with_refs(&"2.2.2.2".parse().unwrap(), &[&ref_deny], &[]),
        AccessDecision::DenyByGroup(_)
    ));
    // inline deny IP still blocked
    assert!(matches!(
        config.check_ip_access_with_refs(&"1.1.1.1".parse().unwrap(), &[&ref_deny], &[]),
        AccessDecision::DenyByGroup(_)
    ));
    // unrelated IP allowed (default Allow)
    assert!(is_allowed(config.check_ip_access_with_refs(
        &"8.8.8.8".parse().unwrap(),
        &[&ref_deny],
        &[]
    )));
}

#[test]
fn allow_refs_missing_denies_all_in_allow_mode() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let config = IpRestrictionConfig {
        allow_refs: Some(vec![EdgionConfigDataRef {
            name: "missing".into(),
            namespace: None,
            ..Default::default()
        }]),
        ..Default::default()
    };
    // allow_mode true, but no resolved allow matchers -> whitelist miss (fail-closed).
    assert!(matches!(
        config.check_ip_access_with_refs(&"8.8.8.8".parse().unwrap(), &[], &[]),
        AccessDecision::DenyByWhitelistMiss
    ));
}
