use super::*;
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::ip_reference_set::IpReferenceSet;
use crate::resources::edgion_plugins::plugin_configs::geo_rule::{GeoDimFacts, GeoRule, GeoRuleKey, GeoRuleSource};

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

/// Parse-only is an HTTP-only mode: it is valid (and needs no `failOpen`)
/// at L7, where the resolved location reaches the access log.
#[test]
fn parse_only_config_is_valid_at_l7_without_fail_open() {
    let cfg = GeoIpLocationConfig::default();
    assert!(cfg.is_parse_only());
    assert!(cfg.check_fields(false).is_none());
}

/// ...and rejected at L4, where it resolves a location with nowhere to put
/// it — `StreamContext` carries no logging sink and there is no L4 access
/// log, so the whole plugin would be a no-op. An operator writing this
/// meant something else; say so at admission instead of accepting a
/// configuration that provably cannot do anything.
#[test]
fn parse_only_config_is_rejected_at_l4() {
    let cfg = GeoIpLocationConfig::default();
    let err = cfg.check_fields(true).expect("L4 parse-only must be rejected");
    assert!(err.contains("at least one"), "unexpected error: {err}");
}

#[test]
fn rule_mode_without_fail_open_is_rejected() {
    let cfg = GeoIpLocationConfig {
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
        }],
        ..Default::default()
    };
    let err = cfg.check_fields(false).unwrap();
    assert!(err.contains("failOpen"));
}

#[test]
fn rule_mode_with_fail_open_is_valid() {
    let mut cfg = GeoIpLocationConfig {
        fail_open: Some(false),
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
        }],
        ..Default::default()
    };
    assert!(cfg.validate_and_init(false).is_ok());
}

#[test]
fn remote_ip_rejected_in_stream_layer() {
    let cfg = GeoIpLocationConfig {
        fail_open: Some(false),
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::RemoteIp, GeoRuleKey::Country, &["RU"])],
        }],
        ..Default::default()
    };
    assert!(cfg.check_fields(false).is_none());
    assert!(cfg.check_fields(true).unwrap().contains("RemoteIp"));
}

#[test]
fn deny_beats_allow_and_allow_miss_denies_under_default_allow() {
    let mut cfg = GeoIpLocationConfig {
        invalid_input: false,
        fail_open: Some(false),
        default_action: DefaultAction::Allow,
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: Some("block-moscow".into()),
            rules: vec![
                rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::City, &["Moscow"]),
                rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"]),
            ],
        }],
        allow_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: Some("office".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
        }],
    };
    cfg.validate_and_init(false).unwrap();

    // Deny hit dominates even though it's also outside the allow set.
    let deny_ctx = GeoRuleContext {
        direct_peer: GeoDimFacts {
            country: Some("RU"),
            city: Some("Moscow"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        cfg.decide(&deny_ctx, |_, _, _, ip| cidr_contains(ip)),
        GeoDecision::Deny
    );

    // Neither deny nor allow hit -> allow configured and missed -> Deny.
    let miss_ctx = GeoRuleContext {
        direct_peer_ip: Some("8.8.8.8".parse().unwrap()),
        direct_peer: GeoDimFacts {
            country: Some("US"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        cfg.decide(&miss_ctx, |_, _, _, ip| cidr_contains(ip)),
        GeoDecision::Deny
    );

    // Allow hit -> Allow.
    let allow_ctx = GeoRuleContext {
        direct_peer_ip: Some("10.1.2.3".parse().unwrap()),
        direct_peer: GeoDimFacts {
            country: Some("US"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        cfg.decide(&allow_ctx, |_, _, _, ip| cidr_contains(ip)),
        GeoDecision::Allow
    );
}

#[test]
fn default_action_deny_with_no_rule_groups_denies_everything() {
    // Degenerate but legal: rule groups present only via defaultAction; here
    // we exercise the "no groups at all, default deny" path indirectly by
    // giving a deny group that never matches would be equivalent to
    // defaultAction alone -- assert the plain defaultAction path directly.
    let cfg = GeoIpLocationConfig {
        default_action: DefaultAction::Deny,
        fail_open: Some(false),
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["ZZ"])],
        }],
        ..Default::default()
    };
    let ctx = GeoRuleContext::default();
    assert_eq!(cfg.decide(&ctx, |_, _, _, ip| cidr_contains(ip)), GeoDecision::Deny);
}

/// Regression: an allow-group HIT is terminal and must not fall through
/// to `defaultAction`. The whitelist shape `defaultAction: deny` +
/// `allowRuleGroups: [CN]` is the whole point of a whitelist — if the
/// default were consulted after a hit, the config would reject exactly
/// the traffic it was written to admit, and every whitelist would be
/// silently inert. Matches `evaluate_ip_access_multi`'s
/// `deny > allow > default` priority.
#[test]
fn allow_group_hit_is_terminal_under_default_deny() {
    let cfg = GeoIpLocationConfig {
        default_action: DefaultAction::Deny,
        fail_open: Some(false),
        allow_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: Some("cn-allow".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["CN"])],
        }],
        ..Default::default()
    };
    let hit = GeoRuleContext {
        direct_peer: GeoDimFacts {
            country: Some("CN"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        cfg.decide(&hit, |_, _, _, ip| cidr_contains(ip)),
        GeoDecision::Allow,
        "an allow hit must allow"
    );
    // A miss under the same config still denies (whitelist miss), and it
    // denies because of the miss, not because of `defaultAction`.
    let miss = GeoRuleContext {
        direct_peer: GeoDimFacts {
            country: Some("RU"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(cfg.decide(&miss, |_, _, _, ip| cidr_contains(ip)), GeoDecision::Deny);
}

/// The same allow-hit terminality holds with `defaultAction: allow` — the
/// default is simply never reached once an allow group is configured.
#[test]
fn allow_group_miss_denies_even_under_default_allow() {
    let cfg = GeoIpLocationConfig {
        default_action: DefaultAction::Allow,
        fail_open: Some(false),
        allow_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: Some("cn-allow".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["CN"])],
        }],
        ..Default::default()
    };
    let miss = GeoRuleContext {
        direct_peer: GeoDimFacts {
            country: Some("RU"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(cfg.decide(&miss, |_, _, _, ip| cidr_contains(ip)), GeoDecision::Deny);
}

/// The failure this plugin most needs to be immune to: a mistyped
/// `denyRuleGroup` (singular) used to deserialize into a config with no
/// rule groups at all — i.e. parse-only mode. The operator's deny rules
/// vanished, their `failOpen: false` stopped being required, every request
/// was allowed, and the resource still reported `Accepted: True`.
#[test]
fn mistyped_rule_group_key_is_rejected_not_degraded_to_parse_only() {
    let yaml = "defaultAction: allow\nfailOpen: false\ndenyRuleGroup:\n  - name: g\n    rules:\n      - source: DirectPeerIp\n        key: country\n        values: [\"RU\"]\n";
    let config = yaml_serde::from_str::<GeoIpLocationConfig>(yaml).unwrap();
    assert!(config.check_fields(false).is_some());
    assert!(config.invalid_input);
}

/// Parse-only remains reachable the only way it should be: by actually
/// writing an empty config.
#[test]
fn explicitly_empty_config_is_still_parse_only() {
    let cfg = yaml_serde::from_str::<GeoIpLocationConfig>("{}").expect("an empty config stays valid");
    assert!(cfg.is_parse_only());
    assert!(cfg.check_fields(false).is_none());
}

#[test]
fn cidr_only_config_needs_no_geo_facts() {
    let cfg = GeoIpLocationConfig {
        fail_open: Some(false),
        allow_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: Some("office".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
        }],
        ..Default::default()
    };
    assert!(!cfg.source_needs_geo_facts(GeoRuleSource::DirectPeerIp));
    assert!(cfg.source_is_used(GeoRuleSource::DirectPeerIp));
    assert!(!cfg.source_is_used(GeoRuleSource::RemoteIp));
}

#[test]
fn country_rule_needs_geo_facts() {
    let cfg = GeoIpLocationConfig {
        fail_open: Some(false),
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
        }],
        ..Default::default()
    };
    assert!(cfg.source_needs_geo_facts(GeoRuleSource::DirectPeerIp));
}

/// Controller-ingest-shaped call vs. gateway-construction-shaped call must
/// produce byte-identical error text, per invariant #5 in the task brief.
#[test]
fn check_fields_and_validate_and_init_agree_on_error_text() {
    let cfg = GeoIpLocationConfig {
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
        }],
        ..Default::default()
    };
    // Controller-ingest shape: stateless check_fields (no compile).
    let controller_err = cfg.check_fields(false);
    // Gateway-construction shape: validate_and_init (check_fields first, then compile).
    let gateway_err = cfg.clone().validate_and_init(false).err();
    assert_eq!(controller_err, gateway_err);
    assert!(controller_err.unwrap().contains("failOpen"));
}

fn cidr_contains(ip: std::net::IpAddr) -> bool {
    let mut builder = edgion_algo::matcher::radix_ip::IpRadixMatcherBuilder::new();
    builder.insert("10.0.0.0/8", "private").unwrap();
    builder.insert("203.0.113.5/32", "host").unwrap();
    builder.build().unwrap().contains(&ip)
}

#[test]
fn config_and_group_unknown_presence_cannot_turn_invalid_policy_into_parse_only() {
    for value in [
        serde_json::json!(null),
        serde_json::json!([]),
        serde_json::json!({"rejected":"payload"}),
    ] {
        let config: GeoIpLocationConfig =
            serde_json::from_value(serde_json::json!({"invalidInput":false,"denyRuleGroup":value})).unwrap();
        assert!(config.check_fields(false).is_some());
        let roundtrip: GeoIpLocationConfig = serde_json::from_value(serde_json::to_value(config).unwrap()).unwrap();
        assert!(roundtrip.check_fields(false).is_some());
        let group:GeoRuleGroup=serde_json::from_value(serde_json::json!({"rules":[{"source":"DirectPeerIp","key":"country","values":["US"]}],"geoipRefs":null,"invalidInput":false})).unwrap();
        assert!(group.check_fields(false).is_some());
        let encoded = serde_json::to_value(group).unwrap();
        assert!(encoded.get("geoipRefs").is_none());
        assert_eq!(encoded["invalidInput"], true);
    }
    let clean: GeoIpLocationConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(clean.check_fields(false).is_none());
}
