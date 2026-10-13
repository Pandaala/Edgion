use super::*;
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::ip_reference_set::IpReferenceSet;

fn rule(source: GeoRuleSource, key: GeoRuleKey, values: &[&str]) -> GeoRule {
    GeoRule {
        source,
        key,
        values: if key == GeoRuleKey::Cidr {
            Vec::new()
        } else {
            values.iter().map(|s| s.to_string()).collect()
        },
        cidr: if key == GeoRuleKey::Cidr {
            IpReferenceSet {
                ip_refs: values
                    .iter()
                    .map(|_| EdgionConfigDataRef {
                        name: "networks".into(),
                        namespace: None,
                        ref_denied: None,
                    })
                    .collect(),
                ..Default::default()
            }
        } else {
            Default::default()
        },
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
    assert!(err.contains("nonempty values"), "unexpected error: {err}");
    assert!(r.compile(false).is_err());
    assert!(!r.matches(None, &facts(None), cidr_contains));
    assert!(!r.matches(None, &facts(Some("RU")), cidr_contains));
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
    let rule = serde_yaml::from_str::<GeoRule>("source: DirectPeerIp\nkey: country\nvaluez: [\"RU\"]\n").unwrap();
    assert!(rule.field_error(false).is_some());
    assert!(rule.values.is_empty());
}

#[test]
fn unknown_field_in_group_is_rejected() {
    let group = serde_yaml::from_str::<GeoRuleGroup>("name: g\nrulez: []\n").unwrap();
    assert!(group.check_fields(false).is_some());
}

#[test]
fn missing_dimension_does_not_match_a_constrained_rule() {
    let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"]);
    r.compile(false).unwrap();
    assert!(!r.matches(None, &facts(None), cidr_contains));
}

#[test]
fn country_uppercased_at_compile() {
    let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["ru"]);
    r.compile(false).unwrap();
    assert_eq!(r.values, vec!["RU".to_string()]);
    assert!(r.matches(None, &facts(Some("RU")), cidr_contains));
}

#[test]
fn cidr_matches_by_membership() {
    let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"]);
    r.compile(false).unwrap();
    let ip: IpAddr = "10.1.2.3".parse().unwrap();
    assert!(r.matches(Some(ip), &GeoDimFacts::default(), cidr_contains));
    let other: IpAddr = "192.168.1.1".parse().unwrap();
    assert!(!r.matches(Some(other), &GeoDimFacts::default(), cidr_contains));
}

#[test]
fn bare_ip_is_accepted_as_cidr_shorthand() {
    let mut r = rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["203.0.113.5"]);
    r.compile(false).unwrap();
    let ip: IpAddr = "203.0.113.5".parse().unwrap();
    assert!(r.matches(Some(ip), &GeoDimFacts::default(), cidr_contains));
}

#[test]
fn invalid_cidr_is_rejected() {
    let rule: GeoRule =
        serde_json::from_value(serde_json::json!({"source":"DirectPeerIp", "key":"cidr", "values":["not-an-ip"]}))
            .unwrap();
    assert!(rule.field_error(false).is_some());
    assert!(rule.values.is_empty(), "rejected addresses never persist");
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
        invalid_input: false,
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
    assert!(group.matches(&ctx_hit, |_, ip| cidr_contains(ip)));

    // Country matches but city doesn't -> AND fails.
    let ctx_miss = GeoRuleContext {
        direct_peer: GeoDimFacts {
            country: Some("RU"),
            city: Some("Kazan"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(!group.matches(&ctx_miss, |_, ip| cidr_contains(ip)));
}

#[test]
fn empty_rule_group_is_rejected() {
    let g = GeoRuleGroup {
        invalid_input: false,
        name: None,
        rules: vec![],
    };
    assert!(g.check_fields(false).is_some());
}

fn cidr_contains(ip: std::net::IpAddr) -> bool {
    let mut builder = edgion_algo::matcher::radix_ip::IpRadixMatcherBuilder::new();
    builder.insert("10.0.0.0/8", "private").unwrap();
    builder.insert("203.0.113.5/32", "host").unwrap();
    builder.build().unwrap().contains(&ip)
}

#[test]
fn removed_and_unknown_field_presence_is_transported_without_payload() {
    for (key, value) in [
        ("values", serde_json::json!(null)),
        ("values", serde_json::json!([])),
        ("values", serde_json::json!(["10.0.0.0/8"])),
        ("geoipRefs", serde_json::json!(null)),
        ("ipRefz", serde_json::json!([])),
    ] {
        let mut wire = serde_json::json!({"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"networks"}],"invalidInput":false});
        wire[key] = value;
        let rule: GeoRule = serde_json::from_value(wire).unwrap();
        assert!(rule.field_error(false).is_some(), "presence of {key} is invalid");
        assert!(rule.values.is_empty());
        let encoded = serde_json::to_value(&rule).unwrap();
        assert_eq!(encoded["invalidInput"], true);
        assert!(encoded.get(key).is_none() || key == "invalidInput");
        let roundtrip: GeoRule = serde_json::from_value(encoded).unwrap();
        assert!(roundtrip.field_error(false).is_some());
    }
    let clean: GeoRule = serde_json::from_value(
        serde_json::json!({"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"networks"}]}),
    )
    .unwrap();
    assert!(clean.field_error(false).is_none());
}

#[test]
fn geographic_refs_and_unknown_source_key_are_static_invalid() {
    for refs in [
        serde_json::json!(null),
        serde_json::json!([]),
        serde_json::json!([{"name":"networks"}]),
    ] {
        let rule: GeoRule = serde_json::from_value(
            serde_json::json!({"source":"DirectPeerIp","key":"country","values":["US"],"ipRefs":refs}),
        )
        .unwrap();
        assert!(rule.field_error(false).is_some());
        let roundtrip: GeoRule = serde_json::from_value(serde_json::to_value(rule).unwrap()).unwrap();
        assert!(roundtrip.field_error(false).is_some());
    }
    for (source, key) in [("TypoPeer", "country"), ("DirectPeerIp", "typo-key")] {
        let rule: GeoRule =
            serde_json::from_value(serde_json::json!({"source":source,"key":key,"values":["US"]})).unwrap();
        assert!(rule.field_error(false).is_some());
    }
}
