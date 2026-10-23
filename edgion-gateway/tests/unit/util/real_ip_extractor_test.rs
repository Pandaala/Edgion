use super::*;

fn private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private(),
        IpAddr::V6(ip) => ip.to_string().starts_with("2001:db8:"),
    }
}

#[test]
fn canonical_resolution_returns_remote_ip_and_downstream_trust() {
    let extractor = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, None);
    for (peer, header, expected, trusted) in [
        (
            "192.168.1.254:45678",
            Some("203.0.113.1, 192.168.1.1"),
            "203.0.113.1",
            true,
        ),
        ("[2001:db8::2]:443", None, "2001:db8::2", true),
        ("[2001:db9::2]:443", Some("203.0.113.2"), "2001:db9::2", false),
        ("not-an-address", Some("203.0.113.3"), "0.0.0.0", false),
        (
            "192.168.1.254",
            Some(" , invalid, 203.0.113.4:123, 10.0.0.1"),
            "10.0.0.1",
            true,
        ),
        ("192.168.1.254", Some("invalid, "), "192.168.1.254", true),
    ] {
        let result = extractor.resolve_real_ip(peer, header, private);
        assert_eq!(result.remote_ip.to_string(), expected);
        assert_eq!(result.downstream_trusted, trusted);
    }
}

#[test]
fn recursive_all_trusted_returns_leftmost_parsed_hop() {
    let extractor = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, None);
    assert_eq!(
        extractor
            .resolve_real_ip("192.168.1.254", Some("10.0.0.9, 192.168.1.1"), private)
            .remote_ip
            .to_string(),
        "10.0.0.9"
    );
}

#[test]
fn non_recursive_untrusted_rightmost_and_trusted_peer_fallback() {
    for cap in [None, Some(0), Some(1)] {
        let extractor = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), false, cap);
        assert_eq!(
            extractor
                .resolve_real_ip("10.0.0.1", Some("1.2.3.4, 10.0.0.99"), private)
                .remote_ip
                .to_string(),
            "10.0.0.1"
        );
        assert_eq!(
            extractor
                .resolve_real_ip("10.0.0.1", Some("10.0.0.99, 1.2.3.4"), private)
                .remote_ip
                .to_string(),
            "1.2.3.4"
        );
    }
}

#[test]
fn test_max_trusted_hops_caps_recursive_traversal() {
    for (cap, expected) in [
        (None, "1.2.3.4"),
        (Some(0), "10.0.0.1"),
        (Some(1), "10.0.0.1"),
        (Some(2), "10.0.0.50"),
        (Some(3), "10.0.0.99"),
    ] {
        let extractor = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, cap);
        assert_eq!(
            extractor
                .resolve_real_ip(
                    "10.0.0.1:45678",
                    Some("1.2.3.4, 10.0.0.99, 10.0.0.50, 10.0.0.1"),
                    private
                )
                .remote_ip
                .to_string(),
            expected
        );
    }
}

#[test]
fn membership_callback_covers_effective_peer_and_each_valid_hop_once() {
    let extractor = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, None);
    let mut seen = Vec::new();
    let result = extractor.resolve_real_ip("10.0.0.1", Some("1.2.3.4, bad, 10.0.0.99"), |ip| {
        seen.push(*ip);
        private(ip)
    });
    assert_eq!(result.remote_ip.to_string(), "1.2.3.4");
    assert_eq!(
        seen,
        ["10.0.0.1", "10.0.0.99", "1.2.3.4"].map(|ip| ip.parse::<IpAddr>().unwrap())
    );
}

#[test]
fn unavailable_whole_trust_ignores_forwarded_input() {
    let extractor = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, None);
    let result = extractor.resolve_real_ip("192.168.1.254:45678", Some("203.0.113.7"), |_| false);
    assert_eq!(result.remote_ip.to_string(), "192.168.1.254");
    assert!(!result.downstream_trusted);
}

#[test]
fn non_utf8_header_falls_back_to_the_trusted_peer() {
    let header = http::HeaderValue::from_bytes(&[0xff]).unwrap();
    let result = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, None).resolve_real_ip(
        "192.168.1.254",
        header.to_str().ok(),
        private,
    );
    assert!(result.downstream_trusted);
    assert_eq!(result.remote_ip.to_string(), "192.168.1.254");
}

#[test]
fn test_client_not_trusted() {
    let result = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, None).resolve_real_ip(
        "192.168.1.1:45678",
        Some("203.0.113.1"),
        |ip| matches!(ip, IpAddr::V4(ip) if ip.octets()[0] == 10),
    );
    assert_eq!(result.remote_ip.to_string(), "192.168.1.1");
}

#[test]
fn test_recursive_extraction() {
    let result = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, None).resolve_real_ip(
        "192.168.1.254:45678",
        Some("203.0.113.1, 198.51.100.2, 192.168.1.1"),
        |ip| private(ip) || *ip == "198.51.100.2".parse::<IpAddr>().unwrap(),
    );
    assert_eq!(result.remote_ip.to_string(), "203.0.113.1");
}

#[test]
fn test_non_recursive_extraction() {
    let extractor = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), false, None);
    assert_eq!(
        extractor
            .resolve_real_ip(
                "192.168.1.254:45678",
                Some("203.0.113.1, 198.51.100.2, 192.168.1.1"),
                private
            )
            .remote_ip
            .to_string(),
        "192.168.1.254"
    );
    assert_eq!(
        extractor
            .resolve_real_ip("192.168.1.254:45678", Some("203.0.113.1, 8.8.8.8"), private)
            .remote_ip
            .to_string(),
        "8.8.8.8"
    );
}

#[test]
fn test_all_ips_trusted_fallback() {
    let result = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, None).resolve_real_ip(
        "192.168.1.254:45678",
        Some("203.0.113.1, 198.51.100.2"),
        |_| true,
    );
    assert_eq!(result.remote_ip.to_string(), "203.0.113.1");
}

#[test]
fn test_zero_max_trusted_hops_stops_at_first_trusted_header_ip() {
    let result = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, Some(0)).resolve_real_ip(
        "10.0.0.1:45678",
        Some("1.2.3.4, 10.0.0.50, 10.0.0.99"),
        private,
    );
    assert_eq!(result.remote_ip.to_string(), "10.0.0.99");
}

#[test]
fn test_max_trusted_hops_does_not_block_real_client_within_window() {
    let result = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, Some(3)).resolve_real_ip(
        "10.0.0.99:45678",
        Some("1.2.3.4, 10.0.0.99"),
        private,
    );
    assert_eq!(result.remote_ip.to_string(), "1.2.3.4");
}

#[test]
fn test_max_trusted_hops_ignored_in_non_recursive_mode() {
    let result = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), false, Some(1)).resolve_real_ip(
        "10.0.0.1:45678",
        Some("1.2.3.4, 10.0.0.99"),
        private,
    );
    assert_eq!(result.remote_ip.to_string(), "10.0.0.1");
}

#[test]
fn forwarded_ipv6_parser_preserves_recursive_hops_and_skips_socket_forms() {
    let extractor = RealIpExtractor::new("X-Forwarded-For".parse().unwrap(), true, None);
    let result = extractor.resolve_real_ip(
        "[2001:db8::2]:443",
        Some("2001:db9::1, [2001:db9::2]:123, 2001:db8::3"),
        private,
    );
    assert!(result.downstream_trusted);
    assert_eq!(result.remote_ip.to_string(), "2001:db9::1");
}

#[test]
fn typed_resolution_avoids_text_allocation_and_preserves_projection() {
    for (recursive, cap, peer, header, expected) in [
        (
            true,
            None,
            "192.168.1.1",
            Some("203.0.113.1, 192.168.1.2"),
            "203.0.113.1",
        ),
        (
            true,
            None,
            "192.168.1.1",
            Some("::ffff:203.0.113.1"),
            "::ffff:203.0.113.1",
        ),
        (true, None, "192.168.1.1", Some("2001:db8::1"), "2001:db8::1"),
        (
            true,
            Some(0),
            "192.168.1.1",
            Some("203.0.113.1, 192.168.1.2"),
            "192.168.1.2",
        ),
        (
            false,
            None,
            "192.168.1.1",
            Some("203.0.113.1, 192.168.1.2"),
            "192.168.1.1",
        ),
        (true, None, "192.168.1.1", Some("invalid"), "192.168.1.1"),
        (true, None, "[::ffff:203.0.113.1]:443", None, "::ffff:203.0.113.1"),
        (true, None, "", None, "0.0.0.0"),
    ] {
        let extractor = RealIpExtractor::new("x-forwarded-for".parse().unwrap(), recursive, cap);
        let expected = expected.parse::<IpAddr>().unwrap();
        let (result, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            extractor.resolve_real_ip(peer, header, |ip| matches!(ip, IpAddr::V4(ip) if ip.is_private()))
        });
        assert_eq!(result.remote_ip, expected);
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
        let old_projection = crate::ctx::canonicalize_ip_str(&expected.to_string()).unwrap();
        assert_eq!(crate::ctx::canonicalize_ip(result.remote_ip), old_projection);
    }
}
