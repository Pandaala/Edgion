use super::*;
use crate::matcher::ip_cidr_helpers::{build_grouped_matcher_partial, evaluate_ip_access_multi};
use crate::{AccessDecision, IpGroup};
use serde_json::json;
fn config(value: serde_json::Value) -> ProfileRules {
    serde_json::from_value(value).unwrap()
}
fn group(name: &str, cidrs: &[&str]) -> IpGroup {
    IpGroup {
        name: name.into(),
        description: None,
        cidrs: cidrs.iter().map(|v| (*v).into()).collect(),
    }
}
// Collection admission and membership belong to canonical IpList machinery, independently of ACL wire policy.
fn decide(allow: &[IpGroup], deny: &[IpGroup], ip: &str, default: DefaultAction) -> AccessDecision {
    let allow_matcher = build_grouped_matcher_partial(allow).0;
    let deny_matcher = build_grouped_matcher_partial(deny).0;
    evaluate_ip_access_multi(
        [&deny_matcher],
        [&allow_matcher],
        !allow.is_empty(),
        &default,
        &ip.parse().unwrap(),
    )
}
fn is_allowed(decision: AccessDecision) -> bool {
    matches!(
        decision,
        AccessDecision::AllowByGroup(_) | AccessDecision::Default(DefaultAction::Allow)
    )
}
fn global(value: serde_json::Value) -> GlobalConnectionIpRestrictionConfig {
    serde_json::from_value(value).unwrap()
}
fn valid_global() -> GlobalConnectionIpRestrictionConfig {
    global(
        json!({"enable":true,"activeProfile":"p1","profiles":{"p1":{"allowRefs":[{"name":"office"}],"defaultAction":"deny"}}}),
    )
}
#[test]
fn construction_and_serde_roundtrip() {
    let mut c = valid_global();
    c.description = Some("test".into());
    let back: GlobalConnectionIpRestrictionConfig = serde_json::from_value(serde_json::to_value(c).unwrap()).unwrap();
    assert_eq!(back.active_profile, "p1");
    assert!(back.enable);
    assert_eq!(back.description, Some("test".into()));
}

#[test]
fn validate_empty_profiles_errs() {
    let c = global(json!({"enable":true,"activeProfile":"p1","profiles":{}}));
    assert!(c.validation_error().unwrap().contains("at least one profile"));
}

#[test]
fn validate_active_profile_must_exist_errs() {
    let mut c = valid_global();
    c.active_profile = "nope".into();
    let err = c.validation_error().unwrap();
    assert!(err.contains("activeProfile") && err.contains("nope"));
}

#[test]
fn validate_profile_requires_allow_or_deny() {
    let c = global(json!({"enable":true,"activeProfile":"p1","profiles":{"p1":{}}}));
    let err = c.validation_error().unwrap();
    assert!(err.contains("At least one") && err.contains("allowRefs"));
}

#[test]
fn validate_duplicate_group_names_is_partial_good() {
    let a = [group("office", &["192.168.1.0/24"]), group("office", &["10.0.0.0/8"])];
    let (matcher, errors) = build_grouped_matcher_partial(&a);
    assert!(!errors.is_empty());
    assert!(matcher.contains(&"192.168.1.50".parse().unwrap()));
    assert!(!matcher.contains(&"10.0.0.1".parse().unwrap()));
}

#[test]
fn validate_empty_cidrs_group_is_partial_good() {
    let (matcher, errors) = build_grouped_matcher_partial(&[group("office", &[])]);
    assert!(!errors.is_empty());
    assert!(!matcher.contains(&"192.168.1.50".parse().unwrap()));
}

#[test]
fn validate_bad_cidr_is_partial_good() {
    let (matcher, errors) = build_grouped_matcher_partial(&[group("office", &["bad-cidr"])]);
    assert!(!errors.is_empty());
    assert!(!matcher.contains(&"192.168.1.50".parse().unwrap()));
}

#[test]
fn validate_present_but_empty_allow_list_still_errs() {
    assert!(config(json!({"allowRefs":[]})).validation_error().is_some());
}

#[test]
fn validate_ok_builds_matchers() {
    let c = valid_global();
    assert!(c.validation_error().is_none());
    assert!(c.profiles["p1"].refs.resolved_allow_refs.is_none());
}

#[test]
fn check_ip_allow_only_whitelist_semantics() {
    let a = [group("office", &["192.168.1.0/24"])];
    assert!(is_allowed(decide(&a, &[], "192.168.1.50", DefaultAction::Allow)));
    assert!(matches!(
        decide(&a, &[], "8.8.8.8", DefaultAction::Allow),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn check_ip_deny_wins_over_allow() {
    let a = [group("all", &["10.0.0.0/8"])];
    let d = [group("bad", &["10.0.0.100"])];
    assert!(is_allowed(decide(&a, &d, "10.0.0.1", DefaultAction::Allow)));
    assert!(
        matches!(decide(&a,&d,"10.0.0.100",DefaultAction::Allow),AccessDecision::DenyByGroup(ref g) if &**g=="bad")
    );
}

#[test]
fn check_ip_deny_only_default_allow() {
    let d = [group("bad", &["1.2.3.4"])];
    assert!(matches!(
        decide(&[], &d, "1.2.3.4", DefaultAction::Allow),
        AccessDecision::DenyByGroup(_)
    ));
    assert!(is_allowed(decide(&[], &d, "8.8.8.8", DefaultAction::Allow)));
}

#[test]
fn check_ip_multi_group_union() {
    let a = [
        group("cloudflare", &["173.245.48.0/20"]),
        group("office", &["192.168.1.0/24"]),
    ];
    for (ip, name) in [("173.245.48.1", "cloudflare"), ("192.168.1.50", "office")] {
        assert!(matches!(decide(&a,&[],ip,DefaultAction::Deny),AccessDecision::AllowByGroup(ref g) if &**g==name));
    }
    assert!(matches!(
        decide(&a, &[], "8.8.8.8", DefaultAction::Deny),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn validate_allow_some_empty_errs_cleanly() {
    let c = config(json!({"allow":[],"denyRefs":[{"name":"bad"}]}));
    assert!(c.refs.invalid_input);
    assert!(c.validation_error().unwrap().contains("Inline ACL"));
}

#[test]
fn profile_allow_mode_true_when_only_allow_refs_set() {
    assert!(config(json!({"allowRefs":[{"name":"corp"}]})).refs.allow_mode());
}

#[test]
fn profile_refs_only_passes_validate_and_init() {
    let c = global(
        json!({"enable":true,"activeProfile":"p1","profiles":{"p1":{"denyRefs":[{"name":"blk"}],"defaultAction":"deny"}}}),
    );
    assert!(c.validation_error().is_none());
}

#[test]
fn profile_with_refs_deny_union_blocks_referenced_ip() {
    let first = build_grouped_matcher_partial(&[group("first", &["1.1.1.1"])]).0;
    let second = build_grouped_matcher_partial(&[group("reflist", &["2.2.2.2"])]).0;
    for ip in ["1.1.1.1", "2.2.2.2"] {
        assert!(matches!(
            evaluate_ip_access_multi(
                [&first, &second],
                [],
                false,
                &DefaultAction::Allow,
                &ip.parse().unwrap()
            ),
            AccessDecision::DenyByGroup(_)
        ));
    }
    assert!(is_allowed(evaluate_ip_access_multi(
        [&first, &second],
        [],
        false,
        &DefaultAction::Allow,
        &"8.8.8.8".parse().unwrap()
    )));
}
