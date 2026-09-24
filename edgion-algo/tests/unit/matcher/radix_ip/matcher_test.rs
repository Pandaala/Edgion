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
