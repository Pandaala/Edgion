use super::*;

// ===== helpers =====

fn valid_profile(name: &str) -> (String, ProfileRules) {
    (
        name.to_string(),
        ProfileRules {
            allow: Some(vec![IpGroup {
                name: "office".to_string(),
                description: None,
                cidrs: vec!["192.168.1.0/24".to_string()],
            }]),
            deny: None,
            default_action: DefaultAction::Deny,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        },
    )
}

#[test]
fn construction_and_serde_roundtrip() {
    let (k, v) = valid_profile("p1");
    let data = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([(k, v)]),
        description: Some("test".to_string()),
        active_profile_ref: None,
    };
    let j = serde_json::to_value(&data).unwrap();
    let back: GlobalConnectionIpRestrictionConfig = serde_json::from_value(j).unwrap();
    assert_eq!(back.active_profile, "p1");
    assert!(back.enable);
}

fn empty_profile() -> ProfileRules {
    ProfileRules {
        allow: None,
        deny: None,
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    }
}

fn group(name: &str, cidrs: &[&str]) -> IpGroup {
    IpGroup {
        name: name.to_string(),
        description: None,
        cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn validate_empty_profiles_errs() {
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::new(),
        description: None,
        active_profile_ref: None,
    };
    let err = d.validate_and_init().unwrap_err();
    assert!(err.contains("at least one profile"), "{}", err);
}

#[test]
fn validate_active_profile_must_exist_errs() {
    let (_, p) = valid_profile("p1");
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "nope".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    let err = d.validate_and_init().unwrap_err();
    assert!(err.contains("activeProfile") && err.contains("nope"), "{}", err);
}

#[test]
fn validate_profile_requires_allow_or_deny() {
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), empty_profile())]),
        description: None,
        active_profile_ref: None,
    };
    let err = d.validate_and_init().unwrap_err();
    assert!(err.contains("at least one of") && err.contains("allow"), "{}", err);
}

#[test]
fn validate_duplicate_group_names_is_partial_good() {
    // Partial-good: a duplicate group name is keep-first (the later occurrence is
    // skipped, not fatal). The first "office" group's CIDR survives.
    let p = ProfileRules {
        allow: Some(vec![
            group("office", &["192.168.1.0/24"]),
            group("office", &["10.0.0.0/8"]),
        ]),
        deny: None,
        default_action: DefaultAction::Deny,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    assert!(d.validate_and_init().is_ok());
    let p = d.profiles.get("p1").unwrap();
    // First occurrence ("office" with 192.168.1.0/24) survives.
    assert!(is_allowed(p.check_ip(&"192.168.1.50".parse().unwrap())));
    // Second occurrence's CIDR (10.0.0.0/8) was skipped -> whitelist miss.
    assert!(matches!(
        p.check_ip(&"10.0.0.1".parse().unwrap()),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn validate_empty_cidrs_group_is_partial_good() {
    // Partial-good: an inline group with no cidrs is now SKIPPED (not fatal) because
    // the list itself is present-and-non-empty. Only a present-but-empty group LIST
    // (`allow: Some(vec![])`) is still rejected (see validate_allow_some_empty_errs_cleanly).
    let p = ProfileRules {
        allow: Some(vec![group("office", &[])]),
        deny: None,
        default_action: DefaultAction::Deny,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    assert!(d.validate_and_init().is_ok());
    assert!(d.profiles.get("p1").unwrap().allow_matcher.is_some());
}

#[test]
fn validate_bad_cidr_is_partial_good() {
    // Partial-good (CIDR-only leniency): a bad inline CIDR is skipped, not fatal.
    let p = ProfileRules {
        allow: Some(vec![group("office", &["not-an-ip", "10.0.0.0/8"])]),
        deny: None,
        default_action: DefaultAction::Deny,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    assert!(d.validate_and_init().is_ok());
    // The good CIDR survived.
    let p = d.profiles.get("p1").unwrap();
    assert!(is_allowed(p.check_ip(&"10.1.2.3".parse().unwrap())));
}

#[test]
fn validate_present_but_empty_allow_list_still_errs() {
    // Structural guard kept: a present-but-empty allow group LIST is rejected.
    // A valid `deny` is supplied so the at-least-one guard passes and we exercise
    // the empty-allow-list path specifically.
    let mut p = ProfileRules {
        allow: Some(vec![]),
        deny: Some(vec![group("bad", &["1.2.3.4"])]),
        default_action: DefaultAction::Deny,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    assert_eq!(p.validate_and_init(), Err("allow group list is empty".to_string()));
}

#[test]
fn validate_ok_builds_matchers() {
    let (k, v) = valid_profile("p1");
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([(k, v)]),
        description: None,
        active_profile_ref: None,
    };
    d.validate_and_init().unwrap();
    let p = d.profiles.get("p1").unwrap();
    assert!(p.allow_matcher.is_some());
}

fn init(data: &mut GlobalConnectionIpRestrictionConfig) {
    data.validate_and_init().unwrap();
}

/// Maps an `AccessDecision` to a boolean allow/deny for test assertions.
fn is_allowed(decision: AccessDecision) -> bool {
    matches!(
        decision,
        AccessDecision::AllowByGroup(_) | AccessDecision::Default(DefaultAction::Allow)
    )
}

/// Convenience: any kind of deny decision.
fn is_denied(decision: AccessDecision) -> bool {
    matches!(
        decision,
        AccessDecision::DenyByGroup(_)
            | AccessDecision::DenyByWhitelistMiss
            | AccessDecision::Default(DefaultAction::Deny)
    )
}

#[test]
fn check_ip_allow_only_whitelist_semantics() {
    let p = ProfileRules {
        allow: Some(vec![group("office", &["192.168.1.0/24"])]),
        deny: None,
        default_action: DefaultAction::Allow, // irrelevant when allow is present
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    init(&mut d);
    let p = d.profiles.get("p1").unwrap();
    assert!(is_allowed(p.check_ip(&"192.168.1.50".parse().unwrap())));
    // Whitelist mode: unmatched IP -> DenyByWhitelistMiss.
    assert!(matches!(
        p.check_ip(&"8.8.8.8".parse().unwrap()),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn check_ip_deny_wins_over_allow() {
    let p = ProfileRules {
        allow: Some(vec![group("all", &["10.0.0.0/8"])]),
        deny: Some(vec![group("bad", &["10.0.0.100"])]),
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    init(&mut d);
    let p = d.profiles.get("p1").unwrap();
    // 10.0.0.1 matches allow group "all".
    assert!(is_allowed(p.check_ip(&"10.0.0.1".parse().unwrap())));
    // 10.0.0.100 matches deny group "bad" (wins over allow).
    match p.check_ip(&"10.0.0.100".parse().unwrap()) {
        AccessDecision::DenyByGroup(g) => assert_eq!(&*g, "bad"),
        other => panic!("expected DenyByGroup(bad), got {:?}", other),
    }
    // 8.8.8.8 outside allow -> DenyByWhitelistMiss.
    assert!(matches!(
        p.check_ip(&"8.8.8.8".parse().unwrap()),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn check_ip_deny_only_default_allow() {
    let p = ProfileRules {
        allow: None,
        deny: Some(vec![group("bad", &["1.2.3.4"])]),
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    init(&mut d);
    let p = d.profiles.get("p1").unwrap();
    assert!(is_denied(p.check_ip(&"1.2.3.4".parse().unwrap())));
    assert!(is_allowed(p.check_ip(&"8.8.8.8".parse().unwrap())));
}

#[test]
fn check_ip_multi_group_union() {
    let p = ProfileRules {
        allow: Some(vec![
            group("cloudflare", &["173.245.48.0/20"]),
            group("office", &["192.168.1.0/24"]),
        ]),
        deny: None,
        default_action: DefaultAction::Deny,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    init(&mut d);
    let p = d.profiles.get("p1").unwrap();
    // 173.245.48.1 matches the cloudflare group.
    match p.check_ip(&"173.245.48.1".parse().unwrap()) {
        AccessDecision::AllowByGroup(g) => assert_eq!(&*g, "cloudflare"),
        other => panic!("expected AllowByGroup(cloudflare), got {:?}", other),
    }
    // 192.168.1.50 matches the office group.
    match p.check_ip(&"192.168.1.50".parse().unwrap()) {
        AccessDecision::AllowByGroup(g) => assert_eq!(&*g, "office"),
        other => panic!("expected AllowByGroup(office), got {:?}", other),
    }
    // Outside any allow group -> DenyByWhitelistMiss.
    assert!(matches!(
        p.check_ip(&"8.8.8.8".parse().unwrap()),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn validate_allow_some_empty_errs_cleanly() {
    let p = ProfileRules {
        allow: Some(vec![]), // Some but empty
        deny: Some(vec![group("bad", &["1.2.3.4"])]),
        default_action: DefaultAction::Deny,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    let err = d.validate_and_init().unwrap_err();
    // Should error cleanly mentioning empty group list (not opaque "Failed to build IpRadixMatcher")
    assert!(err.contains("empty"), "error should mention empty: {}", err);
}

#[test]
fn profile_allow_mode_true_when_only_allow_refs_set() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let p = ProfileRules {
        allow: None,
        deny: None,
        default_action: DefaultAction::Deny,
        allow_refs: Some(vec![EdgionConfigDataRef {
            name: "corp".into(),
            namespace: None,
            ..Default::default()
        }]),
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    assert!(p.allow_mode());
}

#[test]
fn profile_refs_only_passes_validate_and_init() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let p = ProfileRules {
        allow: None,
        deny: None,
        default_action: DefaultAction::Deny,
        allow_refs: None,
        deny_refs: Some(vec![EdgionConfigDataRef {
            name: "blk".into(),
            namespace: None,
            ..Default::default()
        }]),
        allow_matcher: None,
        deny_matcher: None,
    };
    let mut d = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "p1".to_string(),
        profiles: HashMap::from([("p1".to_string(), p)]),
        description: None,
        active_profile_ref: None,
    };
    assert!(d.validate_and_init().is_ok());
}

#[test]
fn profile_with_refs_deny_union_blocks_referenced_ip() {
    use crate::matcher::ip_cidr_helpers::build_grouped_matcher;
    let mut p = ProfileRules {
        allow: None,
        deny: Some(vec![group("inline", &["1.1.1.1"])]),
        default_action: DefaultAction::Allow,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    };
    p.validate_and_init().unwrap();
    let ref_deny = build_grouped_matcher(&[group("reflist", &["2.2.2.2"])]).unwrap();
    assert!(matches!(
        p.check_ip_with_refs(&"2.2.2.2".parse().unwrap(), &[&ref_deny], &[]),
        AccessDecision::DenyByGroup(_)
    ));
    assert!(is_allowed(p.check_ip_with_refs(
        &"8.8.8.8".parse().unwrap(),
        &[&ref_deny],
        &[]
    )));
}
