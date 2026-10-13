use super::*;
use crate::matcher::ip_cidr_helpers::{build_grouped_matcher_partial, evaluate_ip_access_multi};
use crate::{AccessDecision, IpGroup};
use serde_json::json;
fn config(value: serde_json::Value) -> IpRestrictionConfig {
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
#[test]
fn test_default_config() {
    let c = IpRestrictionConfig::default();
    assert!(c.refs.allow_refs.is_none());
    assert!(c.refs.deny_refs.is_none());
    assert_eq!(c.ip_source, IpSource::RemoteIp);
    assert_eq!(c.message, None);
    assert_eq!(c.status, 403);
    assert_eq!(c.default_action, DefaultAction::Allow);
}

#[test]
fn test_missing_both_lists() {
    assert!(IpRestrictionConfig::default()
        .validation_error()
        .unwrap()
        .contains("At least one"));
}

#[test]
fn test_invalid_status_code() {
    for status in [100, 101, 199, 600] {
        let c = config(json!({"allowRefs":[{"name":"clients"}],"status":status}));
        assert!(c.validation_error().unwrap().contains("Invalid status code"));
    }
}

#[test]
fn test_allow_list_validation() {
    assert!(config(json!({"allowRefs":[{"name":"clients"}]}))
        .validation_error()
        .is_none());
}

#[test]
fn test_deny_list_validation() {
    assert!(config(json!({"denyRefs":[{"name":"blocked"}]}))
        .validation_error()
        .is_none());
}

#[test]
fn test_invalid_ip_is_partial_good() {
    let (matcher, errors) = build_grouped_matcher_partial(&[group("default", &["192.168.1.256"])]);
    assert!(!errors.is_empty());
    assert!(!matcher.contains(&"192.168.1.1".parse().unwrap()));
}

#[test]
fn test_invalid_cidr_prefix_is_partial_good() {
    let (matcher, errors) = build_grouped_matcher_partial(&[group("default", &["192.168.1.0/33"])]);
    assert!(!errors.is_empty());
    assert!(!matcher.contains(&"192.168.1.1".parse().unwrap()));
}

#[test]
fn validate_and_init_partial_keeps_good_inline_cidr() {
    let (matcher, errors) = build_grouped_matcher_partial(&[group("g", &["10.0.0.0/8", "bad-ip"])]);
    assert!(!errors.is_empty());
    assert!(matcher.contains(&"10.1.2.3".parse().unwrap()));
}

#[test]
fn test_allow_list_access() {
    let a = [group("default", &["192.168.1.0/24", "10.0.0.1"])];
    for ip in ["192.168.1.50", "10.0.0.1"] {
        assert!(is_allowed(decide(&a, &[], ip, DefaultAction::Allow)));
    }
    assert!(matches!(
        decide(&a, &[], "8.8.8.8", DefaultAction::Allow),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn test_deny_list_access() {
    let d = [group("default", &["192.168.1.100", "10.0.0.0/8"])];
    for ip in ["192.168.1.100", "10.0.0.1"] {
        assert!(matches!(
            decide(&[], &d, ip, DefaultAction::Allow),
            AccessDecision::DenyByGroup(_)
        ));
    }
    assert!(is_allowed(decide(&[], &d, "8.8.8.8", DefaultAction::Allow)));
}

#[test]
fn test_allow_deny_combination() {
    let a = [group("a", &["10.0.0.0/8"])];
    let d = [group("b", &["10.0.0.100"])];
    assert!(is_allowed(decide(&a, &d, "10.0.0.50", DefaultAction::Allow)));
    assert!(matches!(
        decide(&a, &d, "10.0.0.100", DefaultAction::Allow),
        AccessDecision::DenyByGroup(_)
    ));
    assert!(matches!(
        decide(&a, &d, "8.8.8.8", DefaultAction::Allow),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn test_default_action_deny() {
    let d = [group("default", &["192.168.1.100"])];
    assert!(matches!(
        decide(&[], &d, "192.168.1.100", DefaultAction::Deny),
        AccessDecision::DenyByGroup(_)
    ));
    assert!(matches!(
        decide(&[], &d, "8.8.8.8", DefaultAction::Deny),
        AccessDecision::Default(DefaultAction::Deny)
    ));
}

#[test]
fn test_ipv6_support() {
    let a = [group("default", &["::1", "2001:db8::/32"])];
    for ip in ["::1", "2001:db8::1"] {
        assert!(is_allowed(decide(&a, &[], ip, DefaultAction::Allow)));
    }
    assert!(matches!(
        decide(&a, &[], "2001:db9::1", DefaultAction::Allow),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn test_empty_allow_list() {
    assert!(config(json!({"allowRefs":[]}))
        .validation_error()
        .unwrap()
        .contains("At least one"));
}

#[test]
fn test_serialization() {
    let c = config(
        json!({"allowRefs":[{"name":"a"}],"denyRefs":[{"name":"b"}],"ipSource":"DirectPeerIp","message":"Access denied","status":404,"defaultAction":"deny"}),
    );
    let back: IpRestrictionConfig = serde_json::from_value(serde_json::to_value(c).unwrap()).unwrap();
    assert_eq!(back.refs.allow_refs.unwrap()[0].name, "a");
    assert_eq!(back.refs.deny_refs.unwrap()[0].name, "b");
    assert_eq!(back.ip_source, IpSource::DirectPeerIp);
    assert_eq!(back.message, Some("Access denied".into()));
    assert_eq!(back.status, 404);
    assert_eq!(back.default_action, DefaultAction::Deny);
}

#[test]
fn whitelist_miss_returns_specific_variant() {
    assert!(matches!(
        decide(
            &[group("office", &["192.168.0.0/16"])],
            &[],
            "10.0.0.1",
            DefaultAction::Allow
        ),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn deny_priority_returns_deny_group() {
    assert!(
        matches!(decide(&[group("allow_all", &["10.0.0.0/8"])],&[group("blocked", &["10.0.0.100/32"])],"10.0.0.100",DefaultAction::Allow),AccessDecision::DenyByGroup(ref g) if &**g=="blocked")
    );
}

#[test]
fn allow_mode_true_when_only_allow_refs_set() {
    assert!(config(json!({"allowRefs":[{"name":"corp"}]})).refs.allow_mode());
}

#[test]
fn refs_only_config_passes_validation() {
    assert!(config(json!({"denyRefs":[{"name":"blocklist"}]}))
        .validation_error()
        .is_none());
}

#[test]
fn with_refs_deny_union_blocks_referenced_ip() {
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

#[test]
fn allow_refs_missing_denies_all_in_allow_mode() {
    let c = config(json!({"allowRefs":[{"name":"missing"}]}));
    assert!(matches!(
        evaluate_ip_access_multi(
            [],
            [],
            c.refs.allow_mode(),
            &c.default_action,
            &"8.8.8.8".parse().unwrap()
        ),
        AccessDecision::DenyByWhitelistMiss
    ));
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
