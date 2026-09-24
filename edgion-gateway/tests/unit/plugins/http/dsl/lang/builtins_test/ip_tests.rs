use super::is_ssrf_blocked_ip_literal;

// DSL policy: one representative per blocked IpClass + an allowed public case.
// Exhaustive per-segment classification is covered by util::net::classify_ip tests.
#[test]
fn dsl_policy_blocks_non_global_classes() {
    assert!(is_ssrf_blocked_ip_literal("127.0.0.1")); // Loopback v4
    assert!(is_ssrf_blocked_ip_literal("::1")); // Loopback v6
    assert!(is_ssrf_blocked_ip_literal("169.254.1.1")); // LinkLocal
    assert!(is_ssrf_blocked_ip_literal("10.0.0.1")); // Private
    assert!(is_ssrf_blocked_ip_literal("fc00::1")); // UniqueLocal
    assert!(is_ssrf_blocked_ip_literal("0.0.0.0")); // Unspecified
}

#[test]
fn dsl_policy_allows_public() {
    assert!(!is_ssrf_blocked_ip_literal("8.8.8.8"));
    assert!(!is_ssrf_blocked_ip_literal("2001:db8::1"));
}

// str-wrapper behavior — DSL-specific, not covered by classify_ip.
#[test]
fn ipv6_zone_id_stripped() {
    assert!(is_ssrf_blocked_ip_literal("fe80::1%eth0")); // link-local with zone ID
    assert!(is_ssrf_blocked_ip_literal("::1%lo")); // loopback with zone ID
}

#[test]
fn hostnames_not_blocked() {
    // Hostnames are not resolved — always returns false.
    assert!(!is_ssrf_blocked_ip_literal("localhost"));
    assert!(!is_ssrf_blocked_ip_literal("example.com"));
    assert!(!is_ssrf_blocked_ip_literal("internal.corp"));
}

#[test]
fn ipv4_mapped_ipv6_via_str() {
    // The str path plus classify_ip's IPv4-mapped normalization together.
    assert!(is_ssrf_blocked_ip_literal("::ffff:192.168.1.1"));
    assert!(!is_ssrf_blocked_ip_literal("::ffff:8.8.8.8")); // mapped public allowed
}

#[test]
fn reqwest_url_host_str_brackets_stripped() {
    // url crate returns "[::ffff:c0a8:101]" (with brackets) for IPv6 hosts;
    // the call site strips brackets before calling is_ssrf_blocked_ip_literal.
    let url = reqwest::Url::parse("http://[::ffff:192.168.1.1]:80").unwrap();
    let raw = url.host_str().unwrap_or("");
    let host = raw.trim_matches(|c: char| c == '[' || c == ']');
    assert!(
        is_ssrf_blocked_ip_literal(host),
        "IPv4-mapped IPv6 not detected as private: {:?}",
        host
    );
}
