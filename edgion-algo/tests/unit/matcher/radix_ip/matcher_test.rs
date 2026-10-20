use super::*;
use std::net::Ipv6Addr;

#[test]
fn test_empty_builder() {
    let builder = IpRadixMatcherBuilder::new();
    let result = builder.build();
    assert!(matches!(result, Err(IpRadixError::EmptyTree)));
}

#[test]
fn test_ipv4_only() {
    let mut builder = IpRadixMatcher::builder();
    builder.insert("192.168.0.0/16", "g1").unwrap();
    builder.insert("10.0.0.0/8", "g2").unwrap();

    let matcher = builder.build().unwrap();

    let ip1: IpAddr = "192.168.1.50".parse().unwrap();
    let ip2: IpAddr = "10.0.0.1".parse().unwrap();
    let ip3: IpAddr = "8.8.8.8".parse().unwrap();

    assert_eq!(matcher.matched_group(&ip1), Some("g1"));
    assert_eq!(matcher.matched_group(&ip2), Some("g2"));
    assert_eq!(matcher.matched_group(&ip3), None);
}

#[test]
fn test_ipv6_only() {
    let mut builder = IpRadixMatcher::builder();
    builder.insert("2001:db8::/32", "g1").unwrap();
    builder.insert("fe80::/10", "g2").unwrap();

    let matcher = builder.build().unwrap();

    let ip1: IpAddr = "2001:db8::1".parse().unwrap();
    let ip2: IpAddr = "fe80::1".parse().unwrap();
    let ip3: IpAddr = "2001:db9::1".parse().unwrap();

    assert_eq!(matcher.matched_group(&ip1), Some("g1"));
    assert_eq!(matcher.matched_group(&ip2), Some("g2"));
    assert_eq!(matcher.matched_group(&ip3), None);
}

#[test]
fn test_mixed_ipv4_and_ipv6() {
    let mut builder = IpRadixMatcher::builder();

    // IPv4 rules
    builder.insert("192.168.0.0/16", "g1").unwrap();
    builder.insert("10.0.0.0/8", "g2").unwrap();

    // IPv6 rules
    builder.insert("2001:db8::/32", "g1").unwrap();
    builder.insert("fe80::/10", "g2").unwrap();

    let matcher = builder.build().unwrap();

    // Test IPv4
    let ipv4_1: IpAddr = "192.168.1.50".parse().unwrap();
    let ipv4_2: IpAddr = "10.0.0.1".parse().unwrap();
    assert_eq!(matcher.matched_group(&ipv4_1), Some("g1"));
    assert_eq!(matcher.matched_group(&ipv4_2), Some("g2"));

    // Test IPv6
    let ipv6_1: IpAddr = "2001:db8::1".parse().unwrap();
    let ipv6_2: IpAddr = "fe80::1".parse().unwrap();
    assert_eq!(matcher.matched_group(&ipv6_1), Some("g1"));
    assert_eq!(matcher.matched_group(&ipv6_2), Some("g2"));
}

#[test]
fn test_longest_prefix_match_mixed() {
    let mut builder = IpRadixMatcher::builder();

    // IPv4: broader g1, specific g2
    builder.insert("192.168.0.0/16", "g1").unwrap();
    builder.insert("192.168.1.100/32", "g2").unwrap();

    // IPv6: broader g1, specific g2
    builder.insert("2001:db8::/32", "g1").unwrap();
    builder.insert("2001:db8::1/128", "g2").unwrap();

    let matcher = builder.build().unwrap();

    // IPv4: general IP -> g1, specific IP -> g2
    let ipv4_general: IpAddr = "192.168.1.50".parse().unwrap();
    let ipv4_specific: IpAddr = "192.168.1.100".parse().unwrap();
    assert_eq!(matcher.matched_group(&ipv4_general), Some("g1"));
    assert_eq!(matcher.matched_group(&ipv4_specific), Some("g2"));

    // IPv6: general IP -> g1, specific IP -> g2
    let ipv6_general: IpAddr = "2001:db8::2".parse().unwrap();
    let ipv6_specific: IpAddr = "2001:db8::1".parse().unwrap();
    assert_eq!(matcher.matched_group(&ipv6_general), Some("g1"));
    assert_eq!(matcher.matched_group(&ipv6_specific), Some("g2"));
}

#[test]
fn test_ipv4_mapped_ipv6() {
    let mut builder = IpRadixMatcher::builder();
    builder.insert("192.168.1.0/24", "g1").unwrap();
    builder.insert("10.0.0.100/32", "g2").unwrap();

    let matcher = builder.build().unwrap();

    let v6_allow: Ipv6Addr = "::ffff:192.168.1.50".parse().unwrap();
    let mapped_allow = IpAddr::V6(v6_allow);

    let v6_deny: Ipv6Addr = "::ffff:10.0.0.100".parse().unwrap();
    let mapped_deny = IpAddr::V6(v6_deny);

    let v6_no_match: Ipv6Addr = "::ffff:8.8.8.8".parse().unwrap();
    let mapped_no_match = IpAddr::V6(v6_no_match);

    assert_eq!(matcher.matched_group(&mapped_allow), Some("g1"));
    assert_eq!(matcher.matched_group(&mapped_deny), Some("g2"));
    assert_eq!(matcher.matched_group(&mapped_no_match), None);
}

#[test]
fn contains_returns_bool_only() {
    let mut builder = IpRadixMatcher::builder();
    builder.insert("192.168.0.0/16", "office").unwrap();
    let matcher = builder.build().unwrap();
    let ip1: IpAddr = "192.168.1.50".parse().unwrap();
    let ip2: IpAddr = "10.0.0.1".parse().unwrap();
    assert!(matcher.contains(&ip1));
    assert!(!matcher.contains(&ip2));
}

#[test]
fn duplicate_cidr_across_groups_errors() {
    let mut builder = IpRadixMatcher::builder();
    builder.insert("10.0.0.0/24", "office").unwrap();
    let err = builder.insert("10.0.0.0/24", "corp").unwrap_err();
    match err {
        IpRadixError::DuplicateCidr {
            cidr,
            existing_label,
            new_label,
        } => {
            assert_eq!(cidr, "10.0.0.0/24");
            assert_eq!(existing_label, "office");
            assert_eq!(new_label, "corp");
        }
        _ => panic!("wrong variant: {:?}", err),
    }
}

#[test]
fn duplicate_cidr_same_group_is_idempotent() {
    let mut builder = IpRadixMatcher::builder();
    builder.insert("10.0.0.0/24", "office").unwrap();
    builder.insert("10.0.0.0/24", "office").unwrap();
}

#[test]
fn matched_group_arc_clones_cheaply() {
    let mut builder = IpRadixMatcher::builder();
    builder.insert("10.0.0.0/8", "corp").unwrap();
    let matcher = builder.build().unwrap();
    let ip: IpAddr = "10.5.5.5".parse().unwrap();
    let a = matcher.matched_group_arc(&ip).unwrap();
    let b = matcher.matched_group_arc(&ip).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(&*a, "corp");
}

#[test]
fn ipv4_mapped_ipv6_resolves_to_v4_label() {
    let mut builder = IpRadixMatcher::builder();
    builder.insert("192.168.1.0/24", "office").unwrap();
    let matcher = builder.build().unwrap();
    let mapped: IpAddr = "::ffff:192.168.1.50".parse().unwrap();
    assert_eq!(matcher.matched_group(&mapped), Some("office"));
}

#[test]
fn too_many_groups_returns_error() {
    let mut builder = IpRadixMatcher::builder();
    // Intern MAX_LABELS distinct names (1..=65534). Each succeeds.
    for i in 1..=MAX_LABELS {
        builder.intern(&format!("g{}", i)).unwrap();
    }
    // The 65535th distinct name must fail.
    let err = builder.intern("one_more_group").unwrap_err();
    match err {
        IpRadixError::TooManyGroups { max_labels } => {
            assert_eq!(max_labels, MAX_LABELS);
        }
        other => panic!("expected TooManyGroups, got {:?}", other),
    }
}

#[test]
fn mapped_cidrs_use_ipv4_prefixes_and_longest_match() {
    let mut builder = IpRadixMatcher::builder();
    for (cidr, group) in [
        ("::ffff:192.0.2.7/96", "all"),
        ("::ffff:192.0.2.7/120", "subnet"),
        ("::ffff:192.0.2.7/128", "host"),
    ] {
        builder.insert(cidr, group).unwrap();
    }
    let matcher = builder.build().unwrap();
    assert!(matcher.ipv6_tree.is_none());
    for (ip, group) in [("192.0.2.7", "host"), ("192.0.2.8", "subnet"), ("192.0.3.7", "all")] {
        for ip in [ip.to_string(), format!("::ffff:{ip}")] {
            assert_eq!(matcher.matched_group(&ip.parse().unwrap()), Some(group));
        }
    }
    assert!(!matcher.contains(&"2001:db8::1".parse().unwrap()));
}

#[test]
fn mapped_native_aliases_share_terminal_ownership_in_both_orders() {
    for (native, mapped) in [
        ("0.0.0.0/0", "::ffff:192.0.2.7/96"),
        ("192.0.2.0/24", "::ffff:192.0.2.7/120"),
        ("192.0.2.7/32", "::ffff:192.0.2.7/128"),
    ] {
        for (first, second) in [(native, mapped), (mapped, native)] {
            let mut builder = IpRadixMatcher::builder();
            builder.insert(first, "first").unwrap();
            builder.insert(second, "first").unwrap();
            let error = builder.insert(second, "second").unwrap_err();
            assert!(
                matches!(error, IpRadixError::DuplicateCidr { cidr, existing_label, new_label }
                if cidr == second && existing_label == "first" && new_label == "second")
            );
            let matcher = builder.build().unwrap();
            assert_eq!(matcher.matched_group(&"192.0.2.7".parse().unwrap()), Some("first"));
        }
    }
}

#[test]
fn non_mapped_ipv6_rules_keep_separate_family_semantics() {
    for (cidr, covered) in [
        ("::ffff:192.0.2.7/95", "::fffe:192.0.2.7"),
        ("::/0", "2001:db8::1"),
        ("::192.0.2.7/128", "::192.0.2.7"),
        ("::1/128", "::1"),
        ("2001:db8::/32", "2001:db8::1"),
    ] {
        let mut builder = IpRadixMatcher::builder();
        builder.insert(cidr, "v6").unwrap();
        let matcher = builder.build().unwrap();
        assert!(matcher.ipv4_tree.is_none());
        assert!(matcher.contains(&covered.parse().unwrap()));
        assert!(!matcher.contains(&"192.0.2.7".parse().unwrap()));
        assert!(!matcher.contains(&"::ffff:192.0.2.7".parse().unwrap()));
    }
    let cidr = "::ffff:192.0.2.7/120";
    assert!(matches!(
        IpCidr::parse(cidr).unwrap(),
        IpCidr::V6 { prefix_len: 120, .. }
    ));
    let mut raw = IpV6RadixBuilder::new();
    raw.insert(cidr, 1).unwrap();
    let tree = raw.freeze().unwrap();
    assert_eq!(tree.match_ipv6("::ffff:192.0.2.8".parse().unwrap()), Some(1));
    assert_eq!(tree.match_ipv6("::ffff:192.0.3.8".parse().unwrap()), None);
}

#[test]
fn membership_preserves_nested_prefix_group_attribution_and_family_boundaries() {
    for with_defaults in [false, true] {
        let mut builder = IpRadixMatcher::builder();
        for (cidr, group) in [
            ("192.0.2.0/24", "v4-subnet"),
            ("192.0.2.7/32", "v4-host"),
            ("2001:db8::/32", "v6-subnet"),
            ("2001:db8::7/128", "v6-host"),
        ] {
            builder.insert(cidr, group).unwrap();
        }
        if with_defaults {
            builder.insert("0.0.0.0/0", "v4-default").unwrap();
            builder.insert("::/0", "v6-default").unwrap();
        }
        let matcher = builder.build().unwrap();
        for (address, specific, fallback) in [
            ("192.0.2.7", Some("v4-host"), "v4-default"),
            ("192.0.2.8", Some("v4-subnet"), "v4-default"),
            ("::ffff:192.0.2.7", Some("v4-host"), "v4-default"),
            ("::ffff:192.0.2.8", Some("v4-subnet"), "v4-default"),
            ("192.0.3.7", None, "v4-default"),
            ("::ffff:192.0.3.7", None, "v4-default"),
            ("2001:db8::7", Some("v6-host"), "v6-default"),
            ("2001:db8::8", Some("v6-subnet"), "v6-default"),
            ("2001:db9::7", None, "v6-default"),
            ("::192.0.2.7", None, "v6-default"),
            ("::1", None, "v6-default"),
        ] {
            let ip = address.parse().unwrap();
            let expected = specific.or(with_defaults.then_some(fallback));
            assert_eq!(matcher.contains(&ip), expected.is_some(), "{address}");
            assert_eq!(matcher.matched_group(&ip), expected, "{address}");
            assert_eq!(matcher.matched_group_arc(&ip).as_deref(), expected, "{address}");
            assert!(!IpRadixMatcher::empty().contains(&ip));
        }
    }
}
