use super::*;
use crate::matcher::ip_cidr_helpers::{build_grouped_matcher_partial, evaluate_ip_access_multi};
use crate::{AccessDecision, IpGroup};
use serde_json::json;
fn config(value: serde_json::Value) -> ConnectionIpRestrictionRules {
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
fn must_have_allow_or_deny() {
    assert!(ConnectionIpRestrictionRules::default().validation_error().is_some());
}

#[test]
fn allow_whitelist_ok() {
    let a = [group("office", &["192.168.1.0/24"])];
    assert!(is_allowed(decide(&a, &[], "192.168.1.50", DefaultAction::Allow)));
    assert!(matches!(
        decide(&a, &[], "8.8.8.8", DefaultAction::Allow),
        AccessDecision::DenyByWhitelistMiss
    ));
}

#[test]
fn deny_wins_over_allow() {
    let a = [group("internal", &["10.0.0.0/8"])];
    let d = [group("blocked", &["10.0.0.100"])];
    assert!(is_allowed(decide(&a, &d, "10.0.0.1", DefaultAction::Allow)));
    assert!(
        matches!(decide(&a,&d,"10.0.0.100",DefaultAction::Allow),AccessDecision::DenyByGroup(ref g) if &**g=="blocked")
    );
}

#[test]
fn legacy_http_fields_captured_for_preparse() {
    for value in [json!(null), json!([]), json!("discarded")] {
        let c = config(json!({"allowRefs":[{"name":"office"}],"ipSource":value,"message":value,"status":value}));
        let mut fields = c.legacy_http_fields();
        fields.sort();
        assert_eq!(fields, vec!["ipSource", "message", "status"]);
        assert!(c.validation_error().is_some());
        let wire = serde_json::to_value(c).unwrap();
        assert!(wire.get("ipSource").is_none());
        assert!(wire.get("message").is_none());
        assert!(wire.get("status").is_none());
        let back: ConnectionIpRestrictionRules = serde_json::from_value(wire).unwrap();
        assert_eq!(back.legacy_http_fields().len(), 3);
    }
}

#[test]
fn clean_config_has_no_unknown_fields() {
    let c = config(json!({"allowRefs":[{"name":"office"}],"defaultAction":"deny"}));
    assert!(c.legacy_http_fields().is_empty());
    assert!(!c.refs.invalid_input);
    assert!(c.validation_error().is_none());
}

#[test]
fn non_legacy_unknown_field_does_not_trigger_legacy_detection() {
    let c = config(json!({"allowRefs":[{"name":"office"}],"typoedField":"oops"}));
    assert!(c.legacy_http_fields().is_empty());
    assert!(c.validation_error().is_none());
    assert!(serde_json::to_value(c).unwrap().get("typoedField").is_none());
}

#[test]
fn deny_only_default_deny_blocks_all_unmatched() {
    let d = [group("bad", &["1.2.3.4"])];
    assert!(matches!(
        decide(&[], &d, "1.2.3.4", DefaultAction::Deny),
        AccessDecision::DenyByGroup(_)
    ));
    assert!(matches!(
        decide(&[], &d, "8.8.8.8", DefaultAction::Deny),
        AccessDecision::Default(DefaultAction::Deny)
    ));
}

#[test]
fn ipv6_allow_and_deny_work() {
    let a = [group("v6-net", &["2001:db8::/32"])];
    let d = [group("v6-bad", &["2001:db8:dead::/48"])];
    assert!(is_allowed(decide(&a, &d, "2001:db8::1", DefaultAction::Allow)));
    assert!(matches!(
        decide(&a, &d, "2001:db8:dead::1", DefaultAction::Allow),
        AccessDecision::DenyByGroup(_)
    ));
    assert!(matches!(
        decide(&a, &d, "2001:db9::1", DefaultAction::Allow),
        AccessDecision::DenyByWhitelistMiss
    ));
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
fn validation_error_both_none_reports_missing_lists() {
    assert_eq!(
        ConnectionIpRestrictionRules::default().validation_error(),
        Some("At least one of 'allowRefs'/'denyRefs' must be non-empty".into())
    );
}

#[test]
fn validation_error_is_stateless_and_keeps_matchers_unset() {
    let c = config(json!({"allowRefs":[{"name":"office"}]}));
    assert!(c.validation_error().is_none());
    assert!(c.refs.resolved_allow_refs.is_none());
    assert!(c.refs.resolved_deny_refs.is_none());
}

#[test]
fn validation_error_tolerates_invalid_cidr() {
    let (matcher, errors) = build_grouped_matcher_partial(&[group("bad", &["not-an-ip"])]);
    assert!(!errors.is_empty());
    assert!(!matcher.contains(&"192.0.2.1".parse().unwrap()));
    assert!(config(json!({"allowRefs":[{"name":"partial-list"}]}))
        .validation_error()
        .is_none());
}

#[test]
fn deny_priority_returns_deny_group() {
    assert!(
        matches!(decide(&[group("allow_all", &["10.0.0.0/8"])],&[group("blocked", &["10.0.0.100/32"])],"10.0.0.100",DefaultAction::Allow),AccessDecision::DenyByGroup(ref g) if &**g=="blocked")
    );
}
