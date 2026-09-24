use super::*;
use edgion_resources::resources::edgion_plugins::RealIpConfig;

fn group(name: &str, cidrs: &[&str]) -> IpGroup {
    IpGroup {
        name: name.to_string(),
        description: None,
        cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn canonical_resolution_returns_remote_ip_and_downstream_trust() {
    let groups = vec![
        group("private", &["192.168.0.0/16"]),
        group("v6-proxy", &["2001:db8::/32"]),
    ];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, None).unwrap();

    let trusted = extractor.resolve_real_ip("192.168.1.254:45678", Some("203.0.113.1, 192.168.1.1"));
    assert!(trusted.downstream_trusted);
    assert_eq!(trusted.remote_ip, "203.0.113.1");

    let missing_header = extractor.resolve_real_ip("[2001:db8::2]:443", None);
    assert!(missing_header.downstream_trusted);
    assert_eq!(missing_header.remote_ip, "2001:db8::2");

    let untrusted = extractor.resolve_real_ip("[2001:db9::2]:443", Some("203.0.113.2"));
    assert!(!untrusted.downstream_trusted);
    assert_eq!(untrusted.remote_ip, "2001:db9::2");

    let invalid = extractor.resolve_real_ip("not-an-address", Some("203.0.113.3"));
    assert!(!invalid.downstream_trusted);
    assert_eq!(invalid.remote_ip, "0.0.0.0");
}

#[test]
fn existing_extraction_entry_points_preserve_canonical_results() {
    let groups = vec![
        group("private", &["192.168.0.0/16"]),
        group("proxy", &["198.51.100.0/24"]),
    ];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, None).unwrap();
    let value = "203.0.113.1, 198.51.100.2, 192.168.1.1";

    let direct = extractor.resolve_real_ip("192.168.1.254:45678", Some(value));
    assert_eq!(
        extractor.extract_from_header_value("192.168.1.254:45678", value),
        direct.remote_ip
    );

    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers.insert_header("X-Forwarded-For", value).unwrap();
    assert_eq!(
        extractor.extract_real_ip("192.168.1.254:45678", &headers),
        direct.remote_ip
    );

    let missing = RequestHeader::build("GET", b"/", None).unwrap();
    assert_eq!(
        extractor.extract_real_ip("192.168.1.254:45678", &missing),
        "192.168.1.254"
    );
}

#[test]
fn non_utf8_header_falls_back_to_the_trusted_peer() {
    let groups = vec![group("private", &["192.168.0.0/16"])];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, None).unwrap();
    let peer = "192.168.1.254";
    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers
        .insert_header(
            "X-Forwarded-For",
            http::HeaderValue::from_bytes(&[0xff]).expect("obs-text is a valid raw header value"),
        )
        .unwrap();

    let canonical = extractor.resolve_real_ip(peer, None);
    assert!(canonical.downstream_trusted);
    assert_eq!(canonical.remote_ip, peer);
    assert_eq!(extractor.extract_real_ip(peer, &headers), canonical.remote_ip);
}

#[test]
fn test_valid_ipv4_single() {
    let config = RealIpConfig {
        trusted_ips: vec![group("default", &["192.168.1.1"])],
        ..Default::default()
    };
    let extractor = RealIpExtractor::new(
        &config.trusted_ips,
        config.real_ip_header.clone(),
        config.recursive,
        config.max_trusted_hops,
    )
    .unwrap();
    assert!(extractor.is_trusted_proxy(&"192.168.1.1".parse().unwrap()));
    assert!(!extractor.is_trusted_proxy(&"192.168.1.2".parse().unwrap()));
}

#[test]
fn test_valid_ipv4_cidr() {
    let config = RealIpConfig {
        trusted_ips: vec![group("a", &["10.0.0.0/8"]), group("b", &["192.168.1.0/24"])],
        ..Default::default()
    };
    let extractor = RealIpExtractor::new(
        &config.trusted_ips,
        config.real_ip_header.clone(),
        config.recursive,
        config.max_trusted_hops,
    )
    .unwrap();
    assert!(extractor.is_trusted_proxy(&"10.0.0.1".parse().unwrap()));
    assert!(extractor.is_trusted_proxy(&"192.168.1.50".parse().unwrap()));
    assert!(!extractor.is_trusted_proxy(&"172.16.0.1".parse().unwrap()));
}

#[test]
fn test_valid_ipv6() {
    let config = RealIpConfig {
        trusted_ips: vec![group("a", &["::1"]), group("b", &["2001:db8::/32"])],
        ..Default::default()
    };
    let extractor = RealIpExtractor::new(
        &config.trusted_ips,
        config.real_ip_header.clone(),
        config.recursive,
        config.max_trusted_hops,
    )
    .unwrap();
    assert!(extractor.is_trusted_proxy(&"::1".parse().unwrap()));
    assert!(extractor.is_trusted_proxy(&"2001:db8::1".parse().unwrap()));
    assert!(!extractor.is_trusted_proxy(&"2001:db9::1".parse().unwrap()));
}

#[test]
fn test_mixed_valid_invalid() {
    // Strict structural validation rejects any invalid CIDR — so a group
    // mixing valid and invalid CIDRs is now a hard error at validate
    // time. Use only valid CIDRs and verify membership via extractor.
    let config = RealIpConfig {
        trusted_ips: vec![group("a", &["10.0.0.0/8"]), group("b", &["172.16.0.0/12"])],
        ..Default::default()
    };
    let extractor = RealIpExtractor::new(
        &config.trusted_ips,
        config.real_ip_header.clone(),
        config.recursive,
        config.max_trusted_hops,
    )
    .unwrap();
    assert!(extractor.is_trusted_proxy(&"10.0.0.1".parse().unwrap()));
    assert!(extractor.is_trusted_proxy(&"172.16.0.1".parse().unwrap()));
}

#[test]
fn test_no_trusted_ips() {
    let extractor = RealIpExtractor::new(&[], "X-Forwarded-For".to_string(), true, None).unwrap();

    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers.insert_header("X-Forwarded-For", "203.0.113.1").unwrap();

    let result = extractor.extract_real_ip("192.168.1.1:45678", &headers);
    assert_eq!(result, "192.168.1.1");
}

#[test]
fn test_client_not_trusted() {
    let groups = vec![group("default", &["10.0.0.0/8"])];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, None).unwrap();

    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers.insert_header("X-Forwarded-For", "203.0.113.1").unwrap();

    // Client 192.168.1.1 is not in 10.0.0.0/8
    let result = extractor.extract_real_ip("192.168.1.1:45678", &headers);
    assert_eq!(result, "192.168.1.1");
}

#[test]
fn test_recursive_extraction() {
    let groups = vec![
        group("private", &["192.168.0.0/16"]),
        group("public", &["198.51.100.0/24"]),
    ];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, None).unwrap();

    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers
        .insert_header("X-Forwarded-For", "203.0.113.1, 198.51.100.2, 192.168.1.1")
        .unwrap();

    // Recursive: 192.168.1.1 (trusted) -> 198.51.100.2 (trusted) -> 203.0.113.1 (NOT trusted)
    let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
    assert_eq!(result, "203.0.113.1");
}

#[test]
fn test_non_recursive_extraction() {
    let groups = vec![
        group("private", &["192.168.0.0/16"]),
        group("public", &["198.51.100.0/24"]),
    ];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), false, None).unwrap();

    // Non-recursive: rightmost 192.168.1.1 is trusted → use the downstream peer.
    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers
        .insert_header("X-Forwarded-For", "203.0.113.1, 198.51.100.2, 192.168.1.1")
        .unwrap();
    let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
    assert_eq!(result, "192.168.1.254");

    // Non-recursive: rightmost 8.8.8.8 is NOT trusted → use it
    let mut headers2 = RequestHeader::build("GET", b"/", None).unwrap();
    headers2
        .insert_header("X-Forwarded-For", "203.0.113.1, 8.8.8.8")
        .unwrap();
    let result2 = extractor.extract_real_ip("192.168.1.254:45678", &headers2);
    assert_eq!(result2, "8.8.8.8");
}

#[test]
fn test_all_ips_trusted_fallback() {
    let groups = vec![group("any", &["0.0.0.0/0"])]; // All IPs trusted
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, None).unwrap();

    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers
        .insert_header("X-Forwarded-For", "203.0.113.1, 198.51.100.2")
        .unwrap();

    // All IPs trusted, recursive: return leftmost IP (Nginx behavior)
    let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
    assert_eq!(result, "203.0.113.1");
}

#[test]
fn test_invalid_cidrs_skipped() {
    let groups = vec![group(
        "mixed",
        &[
            "10.0.0.0/8",
            "invalid-cidr",
            "192.168.0.0/16",
            "300.0.0.0/8",
            "172.16.0.0/12",
        ],
    )];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, None).unwrap();

    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers
        .insert_header("X-Forwarded-For", "203.0.113.1, 10.0.0.1")
        .unwrap();

    let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
    assert_eq!(result, "203.0.113.1");
}

#[test]
fn test_all_invalid_cidrs() {
    let groups = vec![group("bad", &["invalid-cidr", "not-an-ip/24"])];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, None).unwrap();

    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers.insert_header("X-Forwarded-For", "203.0.113.1").unwrap();

    let result = extractor.extract_real_ip("192.168.1.1:45678", &headers);
    assert_eq!(result, "192.168.1.1");
}

/// Mitigates M12: an attacker inside a broadly-trusted CIDR cannot forge
/// a long "trusted" XFF prefix to push the result leftward once a hop cap
/// is in place.
#[test]
fn test_max_trusted_hops_caps_recursive_traversal() {
    let groups = vec![group("internal", &["10.0.0.0/8"])];
    let extractor = RealIpExtractor::new(
        &groups,
        "X-Forwarded-For".to_string(),
        true,
        Some(2), // cap: stop after skipping 2 trusted hops
    )
    .unwrap();

    // Attacker from 10.0.0.1 sends `1.2.3.4, 10.0.0.99, 10.0.0.50, 10.0.0.1`.
    // Right-to-left: skip 10.0.0.1 (hop 1), skip 10.0.0.50 (hop 2 == cap),
    // stop at 10.0.0.50 — do NOT continue to the forged 1.2.3.4.
    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers
        .insert_header("X-Forwarded-For", "1.2.3.4, 10.0.0.99, 10.0.0.50, 10.0.0.1")
        .unwrap();
    let result = extractor.extract_real_ip("10.0.0.1:45678", &headers);
    assert_eq!(
        result, "10.0.0.50",
        "hop cap must stop traversal and return the last trusted IP within cap, not the forged leftmost"
    );
}

#[test]
fn test_zero_max_trusted_hops_stops_at_first_trusted_header_ip() {
    let groups = vec![group("internal", &["10.0.0.0/8"])];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, Some(0)).unwrap();

    let result = extractor.extract_from_header_value("10.0.0.1:45678", "1.2.3.4, 10.0.0.50, 10.0.0.99");
    assert_eq!(result, "10.0.0.99");
}

/// With the cap, a legitimate untrusted IP within the allowed hop window
/// is still returned (the cap is an upper bound, not a forced value).
#[test]
fn test_max_trusted_hops_does_not_block_real_client_within_window() {
    let groups = vec![group("internal", &["10.0.0.0/8"])];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, Some(3)).unwrap();

    // Only 1 trusted hop before reaching 1.2.3.4 (within cap=3).
    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers.insert_header("X-Forwarded-For", "1.2.3.4, 10.0.0.99").unwrap();
    let result = extractor.extract_real_ip("10.0.0.99:45678", &headers);
    assert_eq!(
        result, "1.2.3.4",
        "cap does not prevent returning real untrusted client within window"
    );
}

/// `max_trusted_hops` has no effect in non-recursive mode (rightmost
/// semantics unchanged).
#[test]
fn test_max_trusted_hops_ignored_in_non_recursive_mode() {
    let groups = vec![group("internal", &["10.0.0.0/8"])];
    let extractor = RealIpExtractor::new(
        &groups,
        "X-Forwarded-For".to_string(),
        false,
        Some(1), // set but should be ignored
    )
    .unwrap();

    let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
    headers.insert_header("X-Forwarded-For", "1.2.3.4, 10.0.0.99").unwrap();
    // Non-recursive takes rightmost → 10.0.0.99 is trusted → fall back to the downstream peer.
    let result = extractor.extract_real_ip("10.0.0.1:45678", &headers);
    assert_eq!(result, "10.0.0.1");
}

#[test]
fn matched_trusted_group_returns_name() {
    let groups = vec![group("cloudflare", &["173.245.48.0/20"])];
    let extractor = RealIpExtractor::new(&groups, "X-Forwarded-For".to_string(), true, None).unwrap();
    assert_eq!(
        extractor.matched_trusted_group(&"173.245.48.5".parse().unwrap()),
        Some("cloudflare")
    );
    assert_eq!(extractor.matched_trusted_group(&"8.8.8.8".parse().unwrap()), None);
}
