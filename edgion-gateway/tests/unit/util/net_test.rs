use super::*;

#[test]
fn bracketed_ip_literal_accepts_only_ipv6() {
    assert_eq!(
        parse_ip_literal("[2001:db8::1]"),
        Some(IpAddr::V6("2001:db8::1".parse().unwrap()))
    );
    assert_eq!(parse_ip_literal("[127.0.0.1]"), None);
}

#[test]
fn test_host_without_port() {
    // IPv6 literals keep their brackets (port stripped or not).
    assert_eq!(host_without_port("[::1]:8443"), "[::1]");
    assert_eq!(host_without_port("[::1]"), "[::1]");
    assert_eq!(host_without_port("[2001:db8::1]:443"), "[2001:db8::1]");
    // IPv4 / registered names: strip a single trailing :port.
    assert_eq!(host_without_port("example.com:80"), "example.com");
    assert_eq!(host_without_port("example.com"), "example.com");
    assert_eq!(host_without_port("127.0.0.1:8080"), "127.0.0.1");
    // Preserve the historical treatment of an empty port suffix.
    assert_eq!(host_without_port("example.com:"), "example.com");
    // Non-numeric suffix after ':' is not a port, so it is preserved as-is.
    assert_eq!(host_without_port("host:notaport"), "host:notaport");
    // Malformed IPv6 (no closing bracket) is returned unchanged.
    assert_eq!(host_without_port("[::1"), "[::1");
    // Bare/unbracketed IPv6 literals have >1 colon and no strippable port; the
    // single-colon guard must leave them untouched (not mangle to `2001:db8:`).
    assert_eq!(host_without_port("2001:db8::1"), "2001:db8::1");
    assert_eq!(host_without_port("::1"), "::1");
}

#[test]
fn test_classify_backend_addr_link_local_metadata() {
    // AWS/GCP/Azure cloud metadata endpoint
    let addr = "169.254.169.254:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(
        classify_backend_addr(&addr),
        Some(BlockedBackendReason::LinkLocal),
        "169.254.169.254 (cloud metadata) must be blocked"
    );

    // Broader link-local segment
    let addr = "169.254.0.1:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::LinkLocal));
    let addr = "169.254.255.254:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::LinkLocal));
}

#[test]
fn test_classify_backend_addr_link_local_ipv6() {
    // fe80::/10 covers fe80::…fe bf::…; check both ends of the range
    let addr = "[fe80::1]:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::LinkLocal));

    let addr = "[febf::1]:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::LinkLocal));

    // fec0:: is outside fe80::/10 — not link-local (historical site-local, deprecated, but not blocked)
    let addr = "[fec0::1]:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), None);
}

#[test]
fn test_classify_backend_addr_localhost() {
    let addr = "127.0.0.1:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::Localhost));
    let addr = "[::1]:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::Localhost));
}

#[test]
fn test_classify_backend_addr_rfc1918_allowed() {
    // RFC1918 private ranges must NOT be blocked — they are common cluster-internal backends
    for candidate in [
        "10.0.0.1:80",
        "10.255.255.255:80",
        "172.16.0.1:80",
        "172.31.255.254:80",
        "192.168.1.1:80",
    ] {
        let addr = candidate.parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(
            classify_backend_addr(&addr),
            None,
            "{candidate} must be allowed (RFC1918)"
        );
    }
}

#[test]
fn test_classify_backend_addr_public_allowed() {
    let addr = "8.8.8.8:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), None);
    let addr = "[2001:db8::1]:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), None);
}

#[test]
fn test_classify_backend_addr_ipv4_mapped_ipv6() {
    // IPv4-mapped IPv6 (`::ffff:a.b.c.d`) must be re-classified via the embedded IPv4
    // address; otherwise loopback / link-local targets bypass the guard (F-003).
    let addr = "[::ffff:127.0.0.1]:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(
        classify_backend_addr(&addr),
        Some(BlockedBackendReason::Localhost),
        "::ffff:127.0.0.1 (mapped loopback) must be blocked"
    );

    let addr = "[::ffff:169.254.169.254]:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(
        classify_backend_addr(&addr),
        Some(BlockedBackendReason::LinkLocal),
        "::ffff:169.254.169.254 (mapped cloud metadata) must be blocked"
    );

    // A mapped *public* address must still be allowed — the normalization must not
    // over-block legitimate dual-stack backends.
    let addr = "[::ffff:8.8.8.8]:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(
        classify_backend_addr(&addr),
        None,
        "::ffff:8.8.8.8 (mapped public) must be allowed"
    );

    // Plain `::1` is NOT an IPv4-mapped address (`to_ipv4_mapped()` returns None for it),
    // so it must still be caught by the IPv6 loopback check, not silently downgraded.
    let addr = "[::1]:80".parse::<PingoraSocketAddr>().unwrap();
    assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::Localhost));
}

// resolve_domain tests — kept hermetic by using IP literals and localhost only,
// so they don't depend on a working external resolver.

#[tokio::test]
async fn resolve_domain_ipv4_literal() {
    let addr = resolve_domain("127.0.0.1", 8080).await.expect("ipv4 literal resolves");
    assert_eq!(addr.ip().to_string(), "127.0.0.1");
    assert_eq!(addr.port(), 8080);
}

#[tokio::test]
async fn resolve_domain_ipv6_literal() {
    // Hardened path: IPv6 literals are short-circuited through IpAddr::from_str and never
    // handed to lookup_host as an unbracketed "::1:9090" (which is itself a valid IPv6
    // address and would otherwise rely on resolver last-colon leniency).
    let addr = resolve_domain("::1", 9090).await.expect("ipv6 literal resolves");
    assert_eq!(addr.ip().to_string(), "::1");
    assert_eq!(addr.port(), 9090);

    // A multi-group literal is the ambiguous case the short-circuit guards against:
    // it must resolve to exactly the given address and port, not be re-split on a colon.
    let addr = resolve_domain("2001:db8::1", 8443)
        .await
        .expect("multi-group ipv6 literal resolves");
    assert_eq!(addr.ip().to_string(), "2001:db8::1");
    assert_eq!(addr.port(), 8443);

    // Region endpoint parsing retains brackets around IPv6 authorities.
    // A fully bracketed IP literal must take the same resolver-free path.
    let addr = resolve_domain("[2001:db8::1]", 9443)
        .await
        .expect("bracketed multi-group ipv6 literal resolves");
    assert_eq!(addr.ip().to_string(), "2001:db8::1");
    assert_eq!(addr.port(), 9443);
}

#[tokio::test]
async fn resolve_domain_localhost() {
    let addr = resolve_domain("localhost", 80).await.expect("localhost resolves");
    assert!(addr.ip().is_loopback(), "localhost must resolve to a loopback address");
    assert_eq!(addr.port(), 80);
}

#[tokio::test]
async fn resolve_domain_invalid_returns_connect_error() {
    // Empty host string is rejected at the parse stage by lookup_host before any
    // resolver is consulted, so this stays hermetic even on networks where the
    // ISP/DNS hijacks NXDOMAIN responses (RFC 2606 .invalid is unreliable in
    // practice for that reason).
    let err = resolve_domain("", 80).await.expect_err("must fail for invalid host");
    assert_eq!(err.etype(), &ErrorType::ConnectError);
}

#[test]
fn test_classify_ip_segments() {
    use std::net::IpAddr;
    let cases: &[(&str, IpClass)] = &[
        // Loopback
        ("127.0.0.1", IpClass::Loopback),
        ("127.255.255.255", IpClass::Loopback),
        ("::1", IpClass::Loopback),
        // Link-local (incl. cloud metadata) + fe80::/10 boundaries
        ("169.254.0.1", IpClass::LinkLocal),
        ("169.254.169.254", IpClass::LinkLocal),
        ("fe80::1", IpClass::LinkLocal),
        ("febf::1", IpClass::LinkLocal),
        ("fec0::1", IpClass::Global), // just outside fe80::/10
        // RFC1918 private
        ("10.0.0.1", IpClass::Private),
        ("172.16.0.1", IpClass::Private),
        ("172.31.255.255", IpClass::Private),
        ("192.168.1.1", IpClass::Private),
        // Unique-local fc00::/7 (both ends)
        ("fc00::1", IpClass::UniqueLocal),
        ("fdff::1", IpClass::UniqueLocal),
        ("fd12:3456::1", IpClass::UniqueLocal),
        // Unspecified
        ("0.0.0.0", IpClass::Unspecified),
        ("::", IpClass::Unspecified),
        // Global / public
        ("8.8.8.8", IpClass::Global),
        ("2001:db8::1", IpClass::Global),
        ("2606:4700::1111", IpClass::Global),
    ];
    for (s, want) in cases {
        let ip: IpAddr = s.parse().unwrap();
        assert_eq!(classify_ip(ip), *want, "classify_ip({s})");
    }
}

#[test]
fn test_classify_ip_ipv4_mapped_normalized_first() {
    use std::net::IpAddr;
    // IPv4-mapped IPv6 must be re-classified via the embedded IPv4 address.
    // Without normalizing FIRST, ::ffff:127.0.0.1 would fall through to Global.
    let mapped: &[(&str, IpClass)] = &[
        ("::ffff:127.0.0.1", IpClass::Loopback),
        ("::ffff:169.254.169.254", IpClass::LinkLocal),
        ("::ffff:192.168.1.1", IpClass::Private),
        ("::ffff:0.0.0.0", IpClass::Unspecified),
        ("::ffff:8.8.8.8", IpClass::Global),
    ];
    for (s, want) in mapped {
        let ip: IpAddr = s.parse().unwrap();
        assert_eq!(classify_ip(ip), *want, "mapped classify_ip({s})");
    }
    // `::` must NOT be mistaken for a mapped address (to_ipv4_mapped() == None),
    // so it reaches the unspecified arm rather than being classified as v4.
    assert_eq!(classify_ip("::".parse::<IpAddr>().unwrap()), IpClass::Unspecified);
}
