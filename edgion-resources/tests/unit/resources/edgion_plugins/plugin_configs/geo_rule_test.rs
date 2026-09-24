use super::*;

fn rule(source: GeoRuleSource, key: GeoRuleKey, values: &[&str]) -> GeoRule {
    GeoRule {
        source,
        key,
        values: values.iter().map(|s| s.to_string()).collect(),
        cidr_matcher: None,
    }
}

fn facts(country: Option<&str>) -> GeoDimFacts<'_> {
    GeoDimFacts {
        country,
        ..Default::default()
    }
}

/// An empty `values` is rejected at admission, and — if one is built in
/// Rust anyway — matches nothing rather than everything. The old
/// "empty means unconstrained" reading made a mistyped `valuez:` silently
/// match every request: a total outage in a deny group, an open whitelist
/// in an allow group.
#[test]
fn empty_values_is_rejected_and_matches_nothing() {
    let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &[]);
    let err = r.field_error(false).expect("empty values must be rejected");
    assert!(err.contains("values must not be empty"), "unexpected error: {err}");
    assert!(r.compile(false).is_err());
    assert!(!r.matches(None, &facts(None)));
    assert!(!r.matches(None, &facts(Some("RU"))));
}

/// The same rejection applies to `key: cidr`, whose empty case previously
/// took the very same "unconstrained" shortcut.
#[test]
fn empty_values_is_rejected_for_cidr_too() {
    let r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &[]);
    assert!(r.field_error(false).is_some());
}

/// A mistyped field name must not be swallowed: the whole point of
/// `deny_unknown_fields` on these structs. `valuez` used to deserialize
/// into an empty `values`, silently neutering the rule.
#[test]
fn unknown_field_in_rule_is_rejected() {
    let e = serde_yaml::from_str::<GeoRule>("source: DirectPeerIp\nkey: country\nvaluez: [\"RU\"]\n")
        .expect_err("a typo'd field must be rejected, not swallowed");
    assert!(e.to_string().contains("valuez"), "unexpected error: {e}");
}

#[test]
fn unknown_field_in_group_is_rejected() {
    assert!(
        serde_yaml::from_str::<GeoRuleGroup>("name: g\nrulez: []\n").is_err(),
        "a typo'd group field must be rejected"
    );
}

#[test]
fn missing_dimension_does_not_match_a_constrained_rule() {
    let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"]);
    r.compile(false).unwrap();
    assert!(!r.matches(None, &facts(None)));
}

#[test]
fn country_uppercased_at_compile() {
    let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["ru"]);
    r.compile(false).unwrap();
    assert_eq!(r.values, vec!["RU".to_string()]);
    assert!(r.matches(None, &facts(Some("RU"))));
}

#[test]
fn cidr_matches_by_membership() {
    let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"]);
    r.compile(false).unwrap();
    let ip: IpAddr = "10.1.2.3".parse().unwrap();
    assert!(r.matches(Some(ip), &GeoDimFacts::default()));
    let other: IpAddr = "192.168.1.1".parse().unwrap();
    assert!(!r.matches(Some(other), &GeoDimFacts::default()));
}

#[test]
fn bare_ip_is_accepted_as_cidr_shorthand() {
    let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["203.0.113.5"]);
    r.compile(false).unwrap();
    let ip: IpAddr = "203.0.113.5".parse().unwrap();
    assert!(r.matches(Some(ip), &GeoDimFacts::default()));
}

#[test]
fn invalid_cidr_is_rejected() {
    let r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["not-an-ip"]);
    assert!(r.field_error(false).is_some());
}

#[test]
fn remote_ip_is_rejected_in_l4_context() {
    let r = rule(GeoRuleSource::RemoteIp, GeoRuleKey::Country, &["RU"]);
    assert!(r.field_error(true).unwrap().contains("RemoteIp"));
    assert!(r.field_error(false).is_none());
}

#[test]
fn group_is_and_across_rules() {
    let mut group = GeoRuleGroup {
        name: Some("moscow".into()),
        rules: vec![
            rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::City, &["Moscow"]),
            rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["ru"]),
        ],
    };
    group.compile(false).unwrap();

    let ctx_hit = GeoRuleContext {
        direct_peer: GeoDimFacts {
            country: Some("RU"),
            city: Some("Moscow"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(group.matches(&ctx_hit));

    // Country matches but city doesn't -> AND fails.
    let ctx_miss = GeoRuleContext {
        direct_peer: GeoDimFacts {
            country: Some("RU"),
            city: Some("Kazan"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(!group.matches(&ctx_miss));
}

#[test]
fn empty_rule_group_is_rejected() {
    let g = GeoRuleGroup {
        name: None,
        rules: vec![],
    };
    assert!(g.check_fields(false).is_some());
}
