use super::*;

#[test]
fn probes_are_public() {
    for p in ["/health", "/ready", "/metrics"] {
        assert!(is_public_probe(p), "{p} should be a probe");
        assert!(is_static_public(p), "{p} should be static-public");
    }
    assert!(!is_public_probe("/api/v1/auth/status"));
}

#[test]
fn status_is_the_only_public_auth_endpoint() {
    assert!(is_public_auth_endpoint("/api/v1/auth/status"));
    assert!(is_static_public("/api/v1/auth/status"));
    assert!(!is_public_auth_endpoint("/api/v1/auth/login"));
    assert!(!is_static_public("/api/v1/auth/me"));
}

#[test]
fn business_routes_are_never_public() {
    assert!(!is_static_public("/api/v1/cluster/httproute"));
    assert!(!is_static_public("/api/v1/auth/login"));
}
