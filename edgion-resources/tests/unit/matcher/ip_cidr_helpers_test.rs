use super::*;
use crate::resources::edgion_config_data::IpGroup;

#[test]
fn validate_ipv4_single_ok() {
    assert!(validate_ip_or_cidr("192.168.1.1").is_ok());
}

#[test]
fn validate_ipv4_cidr_ok() {
    assert!(validate_ip_or_cidr("10.0.0.0/8").is_ok());
}

#[test]
fn validate_ipv6_ok() {
    assert!(validate_ip_or_cidr("::1").is_ok());
    assert!(validate_ip_or_cidr("2001:db8::/32").is_ok());
}

#[test]
fn validate_bad_ip_errs() {
    assert!(validate_ip_or_cidr("192.168.1.256").is_err());
}

#[test]
fn validate_bad_cidr_prefix_errs() {
    assert!(validate_ip_or_cidr("192.168.1.0/33").is_err());
}

#[test]
fn to_cidr_single_ipv4_adds_slash_32() {
    assert_eq!(to_cidr_format("10.0.0.1").unwrap(), "10.0.0.1/32");
}

#[test]
fn to_cidr_single_ipv6_adds_slash_128() {
    assert_eq!(to_cidr_format("::1").unwrap(), "::1/128");
}

#[test]
fn to_cidr_passthrough() {
    assert_eq!(to_cidr_format("10.0.0.0/8").unwrap(), "10.0.0.0/8");
}

#[test]
fn validate_structure_ok() {
    let groups = vec![IpGroup {
        name: "office".to_string(),
        description: None,
        cidrs: vec!["10.0.0.0/8".to_string()],
    }];
    assert!(validate_groups_structure(&groups).is_ok());
}

#[test]
fn validate_structure_empty_errs() {
    assert!(validate_groups_structure(&[]).is_err());
}

#[test]
fn validate_structure_duplicate_name_errs() {
    let groups = vec![
        IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
        IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["192.168.0.0/16".into()],
        },
    ];
    let err = validate_groups_structure(&groups).unwrap_err();
    assert!(err.contains("duplicate group name"));
}

#[test]
fn validate_structure_duplicate_cidr_errs() {
    let groups = vec![
        IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
        IpGroup {
            name: "b".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
    ];
    let err = validate_groups_structure(&groups).unwrap_err();
    assert!(err.contains("duplicate CIDR"));
}

#[test]
fn validate_structure_bad_name_charset_errs() {
    let groups = vec![IpGroup {
        name: "bad name!".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into()],
    }];
    assert!(validate_groups_structure(&groups).is_err());
}

#[test]
fn validate_structure_long_name_errs() {
    let long = "a".repeat(64);
    let groups = vec![IpGroup {
        name: long,
        description: None,
        cidrs: vec!["10.0.0.0/8".into()],
    }];
    assert!(validate_groups_structure(&groups).is_err());
}

#[test]
fn validate_structure_empty_cidrs_errs() {
    let groups = vec![IpGroup {
        name: "a".into(),
        description: None,
        cidrs: vec![],
    }];
    assert!(validate_groups_structure(&groups).is_err());
}

#[test]
fn build_grouped_hits_label() {
    let groups = vec![IpGroup {
        name: "office".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into()],
    }];
    let m = build_grouped_matcher(&groups).unwrap();
    let ip: std::net::IpAddr = "10.1.2.3".parse().unwrap();
    assert_eq!(m.matched_group(&ip), Some("office"));
}

#[test]
fn validate_structure_single_char_name_ok() {
    let groups = vec![IpGroup {
        name: "a".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into()],
    }];
    assert!(validate_groups_structure(&groups).is_ok());
}

#[test]
fn validate_structure_two_char_name_ok() {
    let groups = vec![IpGroup {
        name: "ab".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into()],
    }];
    assert!(validate_groups_structure(&groups).is_ok());
}

#[test]
fn build_grouped_subnet_overlap_picks_longer_prefix() {
    let groups = vec![
        IpGroup {
            name: "wide".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
        IpGroup {
            name: "narrow".into(),
            description: None,
            cidrs: vec!["10.0.0.0/24".into()],
        },
    ];
    let m = build_grouped_matcher(&groups).unwrap();
    let ip: std::net::IpAddr = "10.0.0.5".parse().unwrap();
    assert_eq!(m.matched_group(&ip), Some("narrow"));
    let outside: std::net::IpAddr = "10.5.0.0".parse().unwrap();
    assert_eq!(m.matched_group(&outside), Some("wide"));
}

#[test]
fn multi_deny_any_matcher_wins() {
    let d1 = build_grouped_matcher(&[IpGroup {
        name: "a".into(),
        description: None,
        cidrs: vec!["1.1.1.1/32".into()],
    }])
    .unwrap();
    let d2 = build_grouped_matcher(&[IpGroup {
        name: "b".into(),
        description: None,
        cidrs: vec!["2.2.2.2/32".into()],
    }])
    .unwrap();
    let ip: IpAddr = "2.2.2.2".parse().unwrap();
    let dec = evaluate_ip_access_multi([&d1, &d2], [], false, &DefaultAction::Allow, &ip);
    assert!(matches!(dec, AccessDecision::DenyByGroup(g) if &*g == "b"));
}

#[test]
fn multi_allow_mode_miss_is_whitelist_miss() {
    let a = build_grouped_matcher(&[IpGroup {
        name: "office".into(),
        description: None,
        cidrs: vec!["192.168.0.0/16".into()],
    }])
    .unwrap();
    let ip: IpAddr = "8.8.8.8".parse().unwrap();
    let dec = evaluate_ip_access_multi([], [&a], true, &DefaultAction::Allow, &ip);
    assert!(matches!(dec, AccessDecision::DenyByWhitelistMiss));
}

#[test]
fn multi_allow_mode_true_with_empty_allow_denies_all() {
    // allowRefs-configured-but-all-missing case: allow_mode=true, no allow matchers.
    let ip: IpAddr = "8.8.8.8".parse().unwrap();
    let dec = evaluate_ip_access_multi([], [], true, &DefaultAction::Allow, &ip);
    assert!(matches!(dec, AccessDecision::DenyByWhitelistMiss));
}

#[test]
fn multi_not_allow_mode_falls_through_to_default() {
    let ip: IpAddr = "8.8.8.8".parse().unwrap();
    let dec = evaluate_ip_access_multi([], [], false, &DefaultAction::Deny, &ip);
    assert!(matches!(dec, AccessDecision::Default(DefaultAction::Deny)));
}

#[test]
fn multi_allow_union_across_matchers() {
    let a1 = build_grouped_matcher(&[IpGroup {
        name: "g1".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into()],
    }])
    .unwrap();
    let a2 = build_grouped_matcher(&[IpGroup {
        name: "g2".into(),
        description: None,
        cidrs: vec!["192.168.0.0/16".into()],
    }])
    .unwrap();
    let ip: IpAddr = "192.168.1.1".parse().unwrap();
    let dec = evaluate_ip_access_multi([], [&a1, &a2], true, &DefaultAction::Deny, &ip);
    assert!(matches!(dec, AccessDecision::AllowByGroup(g) if &*g == "g2"));
}

#[test]
fn multi_first_source_preserves_group_arc_and_stops_before_lazy_tail() {
    let first = build_grouped_matcher(&[IpGroup {
        name: "first-wide".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into()],
    }])
    .unwrap();
    let later = build_grouped_matcher(&[IpGroup {
        name: "later-narrow".into(),
        description: None,
        cidrs: vec!["10.1.0.0/16".into()],
    }])
    .unwrap();
    let ip: IpAddr = "10.1.2.3".parse().unwrap();
    let expected = first.matched_group_arc(&ip).unwrap();

    for deny_side in [true, false] {
        let sources = [&first, &later];
        let lazy_sources = sources.iter().copied().enumerate().map(|(index, matcher)| {
            assert_eq!(index, 0, "a decisive first source must stop lazy traversal");
            matcher
        });
        let decision = if deny_side {
            evaluate_ip_access_multi(lazy_sources, [], true, &DefaultAction::Allow, &ip)
        } else {
            evaluate_ip_access_multi([], lazy_sources, true, &DefaultAction::Deny, &ip)
        };
        let group = match decision {
            AccessDecision::DenyByGroup(group) if deny_side => group,
            AccessDecision::AllowByGroup(group) if !deny_side => group,
            _ => panic!("the first source must determine the decision"),
        };
        assert_eq!(group.as_ref(), "first-wide");
        assert!(std::sync::Arc::ptr_eq(&group, &expected));
    }
}

#[test]
fn multi_deny_match_does_not_consume_allow_iterator() {
    let deny = build_grouped_matcher(&[IpGroup {
        name: "blocked".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into()],
    }])
    .unwrap();
    let allow = std::iter::from_fn(|| -> Option<&IpRadixMatcher> {
        panic!("deny precedence must avoid traversing allow sources")
    });
    let ip: IpAddr = "10.1.2.3".parse().unwrap();
    assert!(matches!(
        evaluate_ip_access_multi([&deny], allow, true, &DefaultAction::Allow, &ip),
        AccessDecision::DenyByGroup(group) if group.as_ref() == "blocked"
    ));
}

#[test]
fn refs_structure_ok() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let refs = vec![EdgionConfigDataRef {
        name: "corp".into(),
        namespace: None,
        ..Default::default()
    }];
    assert!(validate_refs_structure(&refs).is_ok());
}

#[test]
fn refs_structure_empty_name_errs() {
    use crate::resources::edgion_config_data::EdgionConfigDataRef;
    let refs = vec![EdgionConfigDataRef {
        name: "  ".into(),
        namespace: None,
        ..Default::default()
    }];
    assert!(validate_refs_structure(&refs).is_err());
}

#[test]
fn refs_structure_empty_list_ok() {
    assert!(validate_refs_structure(&[]).is_ok());
}

#[test]
fn collect_errors_empty_list_is_ok() {
    // Empty items is valid (NOT an error) on the partial path.
    assert!(collect_entry_errors(&[]).is_empty());
}

#[test]
fn collect_errors_invalid_cidr_reported_rest_kept() {
    let groups = vec![IpGroup {
        name: "g".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into(), "not-an-ip".into()],
    }];
    let errs = collect_entry_errors(&groups);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].group, "g");
    assert_eq!(errs[0].cidr.as_deref(), Some("not-an-ip"));
    assert!(errs[0].reason.contains("invalid CIDR"));
}

#[test]
fn collect_errors_empty_group_reported() {
    let groups = vec![IpGroup {
        name: "g".into(),
        description: None,
        cidrs: vec![],
    }];
    let errs = collect_entry_errors(&groups);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].group, "g");
    assert!(errs[0].cidr.is_none());
    assert!(errs[0].reason.contains("no cidrs"));
}

#[test]
fn collect_errors_duplicate_cidr_keep_first() {
    let groups = vec![
        IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
        IpGroup {
            name: "b".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
    ];
    let errs = collect_entry_errors(&groups);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].group, "b");
    assert_eq!(errs[0].cidr.as_deref(), Some("10.0.0.0/8"));
    assert!(errs[0].reason.contains("duplicate CIDR"));
}

#[test]
fn collect_errors_duplicate_group_name_keep_first() {
    let groups = vec![
        IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
        IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["192.168.0.0/16".into()],
        },
    ];
    let errs = collect_entry_errors(&groups);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].group, "a");
    assert!(errs[0].reason.contains("duplicate group name"));
}

#[test]
fn collect_errors_invalid_group_name() {
    let groups = vec![IpGroup {
        name: "bad name!".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into()],
    }];
    let errs = collect_entry_errors(&groups);
    assert_eq!(errs.len(), 1);
    assert!(errs[0].reason.contains("invalid group name"));
}

#[test]
fn format_entry_errors_lists_each() {
    let errs = vec![
        EntryError {
            group: "a".into(),
            cidr: Some("x".into()),
            reason: "invalid CIDR: x".into(),
        },
        EntryError {
            group: "b".into(),
            cidr: None,
            reason: "group has no cidrs".into(),
        },
    ];
    let s = format_entry_errors(&errs);
    assert!(s.contains("a"));
    assert!(s.contains("invalid CIDR"));
    assert!(s.contains("b"));
    assert!(s.contains("; "));
}

#[test]
fn partial_one_bad_cidr_keeps_the_rest() {
    let groups = vec![IpGroup {
        name: "office".into(),
        description: None,
        cidrs: vec!["10.0.0.0/8".into(), "not-an-ip".into()],
    }];
    let (m, errs) = build_grouped_matcher_partial(&groups);
    assert_eq!(errs.len(), 1);
    let ip: std::net::IpAddr = "10.1.2.3".parse().unwrap();
    assert_eq!(m.matched_group(&ip), Some("office")); // good CIDR survived
}

#[test]
fn partial_empty_group_skipped_others_survive() {
    let groups = vec![
        IpGroup {
            name: "empty".into(),
            description: None,
            cidrs: vec![],
        },
        IpGroup {
            name: "good".into(),
            description: None,
            cidrs: vec!["192.168.0.0/16".into()],
        },
    ];
    let (m, errs) = build_grouped_matcher_partial(&groups);
    assert_eq!(errs.len(), 1);
    let ip: std::net::IpAddr = "192.168.1.1".parse().unwrap();
    assert_eq!(m.matched_group(&ip), Some("good"));
}

#[test]
fn partial_duplicate_cidr_keep_first_group() {
    let groups = vec![
        IpGroup {
            name: "a".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
        IpGroup {
            name: "b".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
    ];
    let (m, errs) = build_grouped_matcher_partial(&groups);
    assert_eq!(errs.len(), 1);
    let ip: std::net::IpAddr = "10.1.2.3".parse().unwrap();
    assert_eq!(m.matched_group(&ip), Some("a")); // first occurrence wins
}

#[test]
fn partial_empty_items_is_empty_matcher_no_error() {
    let (m, errs) = build_grouped_matcher_partial(&[]);
    assert!(errs.is_empty());
    let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
    assert_eq!(m.matched_group(&ip), None); // Some(empty), matches nothing
}

#[test]
fn partial_all_bad_is_empty_matcher_with_errors() {
    let groups = vec![IpGroup {
        name: "g".into(),
        description: None,
        cidrs: vec!["bad1".into(), "bad2".into()],
    }];
    let (m, errs) = build_grouped_matcher_partial(&groups);
    assert_eq!(errs.len(), 2);
    let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
    assert_eq!(m.matched_group(&ip), None);
}

#[test]
fn partial_duplicate_group_name_keeps_first() {
    // The first "foo" group is valid; the second is a duplicate and must be skipped.
    // The compiled matcher must still match IPs from the first group's CIDR.
    let groups = vec![
        IpGroup {
            name: "foo".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        },
        IpGroup {
            name: "foo".into(),
            description: None,
            cidrs: vec!["192.168.0.0/16".into()],
        },
    ];
    let (m, errs) = build_grouped_matcher_partial(&groups);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].group, "foo");
    assert!(errs[0].reason.contains("duplicate group name"));
    let ip: std::net::IpAddr = "10.1.2.3".parse().unwrap();
    assert_eq!(m.matched_group(&ip), Some("foo")); // first "foo" must survive
}

#[test]
fn collect_errors_intra_group_duplicate_cidr_keep_first() {
    // A single group listing the same CIDR string twice: keep-first means exactly
    // ONE duplicate error (the 2nd occurrence), the first is kept.
    let groups = vec![IpGroup {
        name: "g".into(),
        description: None,
        cidrs: vec!["1.2.3.4/32".into(), "1.2.3.4/32".into()],
    }];
    let errs = collect_entry_errors(&groups);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].group, "g");
    assert_eq!(errs[0].cidr.as_deref(), Some("1.2.3.4/32"));
    assert!(errs[0].reason.contains("duplicate CIDR"));
}

#[test]
fn partial_intra_group_duplicate_cidr_keeps_first() {
    // Same input as above: the builder must drop EXACTLY what collect reports —
    // ONE duplicate, with the first occurrence KEPT (entry NOT vanished). A lossy
    // value-keyed skip set would drop both and silently produce an empty matcher.
    let groups = vec![IpGroup {
        name: "g".into(),
        description: None,
        cidrs: vec!["1.2.3.4/32".into(), "1.2.3.4/32".into()],
    }];
    let (m, errs) = build_grouped_matcher_partial(&groups);
    assert_eq!(errs.len(), 1);
    let ip: std::net::IpAddr = "1.2.3.4".parse().unwrap();
    assert_eq!(m.matched_group(&ip), Some("g")); // first occurrence KEPT
}

#[test]
fn mapped_rules_reach_strict_and_partial_grouped_matchers() {
    for cidr in ["::ffff:192.0.2.7/120", "::ffff:192.0.2.7"] {
        let groups = vec![IpGroup {
            name: "mapped".into(),
            description: None,
            cidrs: vec![cidr.into()],
        }];
        assert!(collect_entry_errors(&groups).is_empty());
        let strict = build_grouped_matcher(&groups).unwrap();
        let (partial, errors) = build_grouped_matcher_partial(&groups);
        assert!(errors.is_empty());
        for matcher in [&strict, &partial] {
            for ip in ["192.0.2.7", "::ffff:192.0.2.7"] {
                assert_eq!(matcher.matched_group(&ip.parse().unwrap()), Some("mapped"));
            }
            assert!(!matcher.contains(&"192.0.3.7".parse().unwrap()));
        }
    }
}

#[test]
fn mapped_aliases_preserve_strict_conflict_and_partial_first_kept() {
    for (first, second) in [
        ("192.0.2.0/24", "::ffff:192.0.2.7/120"),
        ("::ffff:192.0.2.7/120", "192.0.2.0/24"),
    ] {
        let groups = vec![
            IpGroup {
                name: "first".into(),
                description: None,
                cidrs: vec![first.into()],
            },
            IpGroup {
                name: "second".into(),
                description: None,
                cidrs: vec![second.into()],
            },
        ];
        assert!(build_grouped_matcher(&groups).is_err());
        let (partial, errors) = build_grouped_matcher_partial(&groups);
        assert_eq!(errors, collect_entry_errors(&groups));
        assert!(errors.is_empty()); // Existing advisory checks compare textual CIDRs.
        for ip in ["192.0.2.7", "::ffff:192.0.2.7"] {
            assert_eq!(partial.matched_group(&ip.parse().unwrap()), Some("first"));
        }
        let same_group = vec![IpGroup {
            name: "same".into(),
            description: None,
            cidrs: vec![first.into(), second.into()],
        }];
        let strict = build_grouped_matcher(&same_group).unwrap();
        assert_eq!(strict.matched_group(&"192.0.2.7".parse().unwrap()), Some("same"));
    }
}
