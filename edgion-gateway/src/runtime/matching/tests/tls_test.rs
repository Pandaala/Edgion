//! Unit tests for `tls.rs` (see `super`).
use super::*;
use edgion_resources::resources::gateway::{GatewaySpec, GatewayTLSConfig, GatewayTlsMode, Listener};
use kube::api::ObjectMeta;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Helper: build a ListenerKey for test gateways (no addresses -> 0.0.0.0).
fn test_listener_key(port: u16) -> ListenerKey {
    ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port), "HTTPS")
}

fn create_test_gateway(name: &str, namespace: &str, listeners: Vec<Listener>) -> Gateway {
    Gateway {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        spec: GatewaySpec {
            gateway_class_name: "test-class".to_string(),
            listeners: Some(listeners),
            addresses: None,
            tls: None,
            resolved_inbound_proxy_protocol: Default::default(),
        },
        status: None,
    }
}

fn create_test_listener(name: &str, hostname: Option<&str>, tls: Option<GatewayTLSConfig>) -> Listener {
    create_test_listener_with_port(name, 443, hostname, tls)
}

fn create_test_listener_with_port(
    name: &str,
    port: i32,
    hostname: Option<&str>,
    tls: Option<GatewayTLSConfig>,
) -> Listener {
    Listener {
        name: name.to_string(),
        hostname: hostname.map(|s| s.to_string()),
        port,
        protocol: "HTTPS".to_string(),
        tls,
        allowed_routes: None,
    }
}

fn make_tls_config(cert_name: &str, cert_ns: Option<&str>) -> GatewayTLSConfig {
    GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: cert_name.to_string(),
            namespace: cert_ns.map(|s| s.to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    }
}

#[test]
fn test_rebuild_from_gateways() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let listener = create_test_listener(
        "https",
        Some("example.com"),
        Some(make_tls_config("test-cert", Some("default"))),
    );
    let gateway = create_test_gateway("test-gw", "default", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Test exact match
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_some());
    let entry = result.unwrap();
    assert_eq!(entry.gateway_name, "test-gw");
    assert_eq!(entry.gateway_namespace, "default");
    assert_eq!(entry.listener_name, "https");
    assert_eq!(entry.certificate_refs.len(), 1);
}

#[test]
fn test_no_tls_config() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let listener = create_test_listener("http", Some("example.com"), None);
    let gateway = create_test_gateway("test-gw", "default", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Should not find any entry
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_none());
}

#[test]
fn test_no_hostname() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let tls_config = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "test-cert".to_string(),
            namespace: None,
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };

    let listener = create_test_listener("https", None, Some(tls_config));
    let gateway = create_test_gateway("test-gw", "default", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Hostname-less listeners serve as catch-all fallback for any SNI on the same listener.
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_some(), "Catch-all listener should match any SNI");
    let entry = result.unwrap();
    assert_eq!(entry.gateway_name, "test-gw");
    assert_eq!(entry.listener_name, "https");

    // Should also match other arbitrary SNIs
    let result = matcher.match_sni(&key, "anything.foo.bar");
    assert!(result.is_some(), "Catch-all listener should match any SNI");
}

#[test]
fn test_wildcard_match() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let tls_config = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "wildcard-cert".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };

    // Gateway with wildcard hostname
    let listener = create_test_listener("https", Some("*.example.com"), Some(tls_config));
    let gateway = create_test_gateway("wildcard-gw", "default", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Test wildcard match - subdomain should match
    let result = matcher.match_sni(&key, "api.example.com");
    assert!(result.is_some(), "api.example.com should match *.example.com");
    let entry = result.unwrap();
    assert_eq!(entry.gateway_name, "wildcard-gw");
    assert_eq!(entry.certificate_refs[0].name, "wildcard-cert");

    // Another subdomain should also match
    let result = matcher.match_sni(&key, "www.example.com");
    assert!(result.is_some(), "www.example.com should match *.example.com");

    // Root domain should NOT match wildcard
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_none(), "example.com should NOT match *.example.com");

    // RFC 6125 §6.4.3: deeper subdomain MUST NOT match a single-label wildcard
    let result = matcher.match_sni(&key, "a.b.example.com");
    assert!(
        result.is_none(),
        "a.b.example.com should NOT match *.example.com (RFC 6125 §6.4.3)"
    );
}

#[test]
fn test_multiple_gateways_multiple_listeners() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    // Gateway 1: api.example.com
    let tls_config1 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "api-cert".to_string(),
            namespace: Some("prod".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };
    let listener1 = create_test_listener("api-https", Some("api.example.com"), Some(tls_config1));
    let gateway1 = create_test_gateway("api-gateway", "prod", vec![listener1]);

    // Gateway 2: Multiple listeners (www and admin)
    let tls_config2 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "www-cert".to_string(),
            namespace: Some("prod".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };
    let tls_config3 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "admin-cert".to_string(),
            namespace: Some("prod".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };
    let listener2 = create_test_listener("www-https", Some("www.example.com"), Some(tls_config2));
    let listener3 = create_test_listener("admin-https", Some("admin.example.com"), Some(tls_config3));
    let gateway2 = create_test_gateway("web-gateway", "prod", vec![listener2, listener3]);

    matcher.rebuild_from_gateways(&[gateway1, gateway2]);

    // Test api.example.com
    let result = matcher.match_sni(&key, "api.example.com");
    assert!(result.is_some());
    let entry = result.unwrap();
    assert_eq!(entry.gateway_name, "api-gateway");
    assert_eq!(entry.certificate_refs[0].name, "api-cert");

    // Test www.example.com
    let result = matcher.match_sni(&key, "www.example.com");
    assert!(result.is_some());
    let entry = result.unwrap();
    assert_eq!(entry.gateway_name, "web-gateway");
    assert_eq!(entry.listener_name, "www-https");
    assert_eq!(entry.certificate_refs[0].name, "www-cert");

    // Test admin.example.com
    let result = matcher.match_sni(&key, "admin.example.com");
    assert!(result.is_some());
    let entry = result.unwrap();
    assert_eq!(entry.gateway_name, "web-gateway");
    assert_eq!(entry.listener_name, "admin-https");
    assert_eq!(entry.certificate_refs[0].name, "admin-cert");

    // Test unknown domain
    let result = matcher.match_sni(&key, "unknown.example.com");
    assert!(result.is_none());
}

#[test]
fn test_certificate_ref_namespace_fallback() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    // Certificate ref without explicit namespace should use Gateway's namespace
    let tls_config = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "cert-without-ns".to_string(),
            namespace: None, // No namespace specified
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };

    let listener = create_test_listener("https", Some("test.example.com"), Some(tls_config));
    let gateway = create_test_gateway("test-gw", "my-namespace", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    let result = matcher.match_sni(&key, "test.example.com");
    assert!(result.is_some());
    let entry = result.unwrap();

    // Verify Gateway namespace is captured for later Secret lookup
    assert_eq!(entry.gateway_namespace, "my-namespace");
    // Certificate ref has no explicit namespace
    assert!(entry.certificate_refs[0].namespace.is_none());
}

#[test]
fn test_empty_matcher_fast_path() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    // New matcher should be empty
    assert!(matcher.is_empty(), "New matcher should be empty");

    // Match should fail immediately without HashMap lookup
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_none());

    // Rebuild with empty gateway list
    matcher.rebuild_from_gateways(&[]);
    assert!(matcher.is_empty(), "Matcher should still be empty after empty rebuild");

    // Rebuild with gateway that has no TLS
    let listener = create_test_listener("http", Some("example.com"), None);
    let gateway = create_test_gateway("no-tls-gw", "default", vec![listener]);
    matcher.rebuild_from_gateways(&[gateway]);
    assert!(matcher.is_empty(), "Matcher should be empty when gateway has no TLS");

    // Match should still fail fast
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_none());
}

#[test]
fn test_has_entries_flag_transitions() {
    let matcher = GatewayTlsMatcher::new();

    // Initially empty
    assert!(matcher.is_empty());

    // Add TLS config
    let tls_config = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "test-cert".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };
    let listener = create_test_listener("https", Some("example.com"), Some(tls_config));
    let gateway = create_test_gateway("test-gw", "default", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Now has entries
    assert!(
        !matcher.is_empty(),
        "Matcher should have entries after adding TLS config"
    );

    // Remove all gateways
    matcher.rebuild_from_gateways(&[]);

    // Back to empty
    assert!(
        matcher.is_empty(),
        "Matcher should be empty after removing all gateways"
    );
}

#[test]
fn test_listener_dimension_matching() {
    let matcher = GatewayTlsMatcher::new();
    let key_443 = test_listener_key(443);
    let key_8443 = test_listener_key(8443);
    let key_9443 = test_listener_key(9443);

    // Same hostname on different ports with different certs
    let tls_config_443 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "cert-443".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };

    let tls_config_8443 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "cert-8443".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };

    let listener_443 = create_test_listener_with_port("https-443", 443, Some("api.example.com"), Some(tls_config_443));
    let listener_8443 =
        create_test_listener_with_port("https-8443", 8443, Some("api.example.com"), Some(tls_config_8443));

    let gateway = create_test_gateway("multi-port-gw", "default", vec![listener_443, listener_8443]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Test listener on port 443
    let result = matcher.match_sni(&key_443, "api.example.com");
    assert!(result.is_some(), "Should find cert for listener on port 443");
    let entry = result.unwrap();
    assert_eq!(entry.listener_key, key_443);
    assert_eq!(entry.certificate_refs[0].name, "cert-443");

    // Test listener on port 8443
    let result = matcher.match_sni(&key_8443, "api.example.com");
    assert!(result.is_some(), "Should find cert for listener on port 8443");
    let entry = result.unwrap();
    assert_eq!(entry.listener_key, key_8443);
    assert_eq!(entry.certificate_refs[0].name, "cert-8443");

    // Test non-existent listener
    let result = matcher.match_sni(&key_9443, "api.example.com");
    assert!(result.is_none(), "Should not find cert for listener on port 9443");
}

#[test]
fn test_multiple_listeners_different_hostnames() {
    let matcher = GatewayTlsMatcher::new();
    let key_443 = test_listener_key(443);
    let key_8443 = test_listener_key(8443);

    let tls_config_api = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "api-cert".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };

    let tls_config_admin = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "admin-cert".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        secrets: None,
        frontend_validation: None,
        resolved_frontend_ca_secrets: None,
    };

    // Port 443: api.example.com
    let listener_api = create_test_listener_with_port("api-https", 443, Some("api.example.com"), Some(tls_config_api));
    // Port 8443: admin.example.com
    let listener_admin =
        create_test_listener_with_port("admin-https", 8443, Some("admin.example.com"), Some(tls_config_admin));

    let gateway = create_test_gateway("mixed-gw", "default", vec![listener_api, listener_admin]);

    matcher.rebuild_from_gateways(&[gateway]);

    // api.example.com on listener 443
    let result = matcher.match_sni(&key_443, "api.example.com");
    assert!(result.is_some());
    assert_eq!(result.unwrap().certificate_refs[0].name, "api-cert");

    // admin.example.com on listener 8443
    let result = matcher.match_sni(&key_8443, "admin.example.com");
    assert!(result.is_some());
    assert_eq!(result.unwrap().certificate_refs[0].name, "admin-cert");

    // api.example.com on listener 8443 should fail (wrong listener)
    let result = matcher.match_sni(&key_8443, "api.example.com");
    assert!(result.is_none());

    // admin.example.com on listener 443 should fail (wrong listener)
    let result = matcher.match_sni(&key_443, "admin.example.com");
    assert!(result.is_none());
}

#[test]
fn test_catchall_with_hostname_priority() {
    let matcher = GatewayTlsMatcher::new();
    let key_443 = test_listener_key(443);
    let key_8443 = test_listener_key(8443);

    // Listener 1: specific hostname on port 443
    let tls_specific = make_tls_config("specific-cert", Some("default"));
    let listener_specific =
        create_test_listener_with_port("specific-https", 443, Some("api.example.com"), Some(tls_specific));

    // Listener 2: catch-all (no hostname) on same port 443
    let tls_catchall = make_tls_config("catchall-cert", Some("default"));
    let listener_catchall = create_test_listener_with_port("catchall-https", 443, None, Some(tls_catchall));

    let gateway = create_test_gateway("mixed-gw", "default", vec![listener_specific, listener_catchall]);
    matcher.rebuild_from_gateways(&[gateway]);

    // Exact hostname should use the specific cert, not catch-all
    let result = matcher.match_sni(&key_443, "api.example.com");
    assert!(result.is_some());
    assert_eq!(result.unwrap().certificate_refs[0].name, "specific-cert");

    // Unknown hostname should fall back to catch-all
    let result = matcher.match_sni(&key_443, "unknown.example.com");
    assert!(result.is_some(), "Unknown SNI should match catch-all");
    assert_eq!(result.unwrap().certificate_refs[0].name, "catchall-cert");

    // Different listener should not match either
    let result = matcher.match_sni(&key_8443, "unknown.example.com");
    assert!(result.is_none(), "Different listener should not match");
}

#[test]
fn test_programmed_false_listener_not_registered() {
    use edgion_resources::resources::common::Condition;
    use edgion_resources::resources::gateway::{GatewayStatus, ListenerStatus};

    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let tls_config = make_tls_config("server-cert", Some("default"));
    let listener = create_test_listener("https", Some("api.example.com"), Some(tls_config));

    let mut gateway = create_test_gateway("test-gw", "default", vec![listener]);
    gateway.status = Some(GatewayStatus {
        addresses: None,
        conditions: None,
        listeners: Some(vec![ListenerStatus {
            name: "https".to_string(),
            supported_kinds: vec![],
            attached_routes: 0,
            conditions: vec![Condition {
                last_transition_time: "2024-01-01T00:00:00Z".to_string(),
                message: String::new(),
                observed_generation: None,
                reason: "Invalid".to_string(),
                status: "False".to_string(),
                type_: "Programmed".to_string(),
            }],
        }]),
    });

    matcher.rebuild_from_gateways(&[gateway]);

    // Programmed=False listener must not be in the TLS matcher
    let result = matcher.match_sni(&key, "api.example.com");
    assert!(result.is_none(), "Programmed=False listener should not serve TLS");
    assert!(
        matcher.is_empty(),
        "matcher should be empty when only listener is invalid"
    );
}

#[test]
fn test_exact_hostname_not_merged_into_wildcard_bucket() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let tls_wildcard = make_tls_config("wildcard-cert", Some("default"));
    let tls_exact = make_tls_config("exact-cert", Some("default"));

    let listener_wc = create_test_listener("https-wc", Some("*.wildcard.org"), Some(tls_wildcard));
    let listener_exact = create_test_listener("https-exact", Some("fourth-example.wildcard.org"), Some(tls_exact));

    let gateway = create_test_gateway("test-gw", "default", vec![listener_wc, listener_exact]);
    matcher.rebuild_from_gateways(&[gateway]);

    // Exact hostname must match the exact listener, not the wildcard one
    let result = matcher.match_sni(&key, "fourth-example.wildcard.org");
    assert!(result.is_some(), "exact hostname should match");
    let entry = result.unwrap();
    assert_eq!(
        entry.listener_name, "https-exact",
        "exact hostname must select the exact listener, not the wildcard"
    );
    assert_eq!(entry.certificate_refs[0].name, "exact-cert");

    // Other subdomains should still match the wildcard
    let result = matcher.match_sni(&key, "other.wildcard.org");
    assert!(result.is_some());
    assert_eq!(result.unwrap().listener_name, "https-wc");
}

#[test]
fn test_cross_gateway_sni_collision_is_deterministic() {
    // Two Gateways register TLS for the SAME hostname on the SAME listener.
    // The controller's conflict detection is intra-gateway only, so this is not
    // admission-blocked. match_sni must pick the same winner regardless of the
    // order the Gateways arrive in the rebuild slice (no HashMap-order dependence).
    let key = test_listener_key(443);

    let gw_a = create_test_gateway(
        "alpha-gw",
        "team-a",
        vec![create_test_listener(
            "https",
            Some("shared.example.com"),
            Some(make_tls_config("alpha-cert", Some("team-a"))),
        )],
    );
    let gw_b = create_test_gateway(
        "beta-gw",
        "team-b",
        vec![create_test_listener(
            "https",
            Some("shared.example.com"),
            Some(make_tls_config("beta-cert", Some("team-b"))),
        )],
    );

    // Order 1: A then B
    let matcher1 = GatewayTlsMatcher::new();
    matcher1.rebuild_from_gateways(&[gw_a.clone(), gw_b.clone()]);
    let winner1 = matcher1.match_sni(&key, "shared.example.com").unwrap();

    // Order 2: B then A
    let matcher2 = GatewayTlsMatcher::new();
    matcher2.rebuild_from_gateways(&[gw_b, gw_a]);
    let winner2 = matcher2.match_sni(&key, "shared.example.com").unwrap();

    // Same deterministic winner irrespective of input order
    assert_eq!(winner1.gateway_namespace, winner2.gateway_namespace);
    assert_eq!(winner1.gateway_name, winner2.gateway_name);
    // Neither Gateway sets a creationTimestamp (None), so the tiebreak falls back to
    // (namespace, name): "team-a" < "team-b".
    assert_eq!(winner1.gateway_namespace, "team-a");
    assert_eq!(winner1.gateway_name, "alpha-gw");
    assert_eq!(winner1.certificate_refs[0].name, "alpha-cert");
}

#[test]
fn test_cross_gateway_collision_prefers_oldest_creation_timestamp() {
    // The tiebreak is creationTimestamp-first (oldest wins), then namespace/name.
    // Give the lexically-LARGER-named Gateway the OLDER timestamp; it must still win,
    // proving creationTimestamp dominates the name comparison.
    use chrono::{TimeZone, Utc};
    let key = test_listener_key(443);

    let older = Time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
    let newer = Time(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());

    let mut incumbent = create_test_gateway(
        "zzz-gw",
        "team-z",
        vec![create_test_listener(
            "https",
            Some("shared.example.com"),
            Some(make_tls_config("incumbent-cert", Some("team-z"))),
        )],
    );
    incumbent.metadata.creation_timestamp = Some(older);

    let mut newcomer = create_test_gateway(
        "aaa-gw",
        "team-a",
        vec![create_test_listener(
            "https",
            Some("shared.example.com"),
            Some(make_tls_config("newcomer-cert", Some("team-a"))),
        )],
    );
    newcomer.metadata.creation_timestamp = Some(newer);

    for order in [
        vec![incumbent.clone(), newcomer.clone()],
        vec![newcomer.clone(), incumbent.clone()],
    ] {
        let matcher = GatewayTlsMatcher::new();
        matcher.rebuild_from_gateways(&order);
        let winner = matcher.match_sni(&key, "shared.example.com").unwrap();
        assert_eq!(
            winner.gateway_name, "zzz-gw",
            "older Gateway must win regardless of name or input order"
        );
        assert_eq!(winner.certificate_refs[0].name, "incumbent-cert");
    }
}

#[test]
fn test_cross_gateway_wildcard_collision_is_deterministic() {
    // Same scenario for an identical *wildcard* hostname on one listener. Before
    // the get_mut_exact wildcard fix, the second Gateway silently overwrote the
    // first (last-in-slice wins) — non-deterministic by input order.
    let key = test_listener_key(443);

    let gw_a = create_test_gateway(
        "alpha-gw",
        "team-a",
        vec![create_test_listener(
            "https",
            Some("*.example.com"),
            Some(make_tls_config("alpha-cert", Some("team-a"))),
        )],
    );
    let gw_b = create_test_gateway(
        "beta-gw",
        "team-b",
        vec![create_test_listener(
            "https",
            Some("*.example.com"),
            Some(make_tls_config("beta-cert", Some("team-b"))),
        )],
    );

    let matcher1 = GatewayTlsMatcher::new();
    matcher1.rebuild_from_gateways(&[gw_a.clone(), gw_b.clone()]);
    let winner1 = matcher1.match_sni(&key, "foo.example.com").unwrap();

    let matcher2 = GatewayTlsMatcher::new();
    matcher2.rebuild_from_gateways(&[gw_b, gw_a]);
    let winner2 = matcher2.match_sni(&key, "foo.example.com").unwrap();

    assert_eq!(winner1.gateway_name, winner2.gateway_name);
    assert_eq!(winner1.gateway_name, "alpha-gw");
    assert_eq!(winner1.certificate_refs[0].name, "alpha-cert");
}

#[test]
fn test_cross_gateway_catchall_collision_is_deterministic() {
    // Same scenario for hostname-less catch-all listeners colliding on one listener.
    let key = test_listener_key(443);

    let gw_a = create_test_gateway(
        "alpha-gw",
        "team-a",
        vec![create_test_listener(
            "https",
            None,
            Some(make_tls_config("alpha-cert", Some("team-a"))),
        )],
    );
    let gw_b = create_test_gateway(
        "beta-gw",
        "team-b",
        vec![create_test_listener(
            "https",
            None,
            Some(make_tls_config("beta-cert", Some("team-b"))),
        )],
    );

    let matcher1 = GatewayTlsMatcher::new();
    matcher1.rebuild_from_gateways(&[gw_a.clone(), gw_b.clone()]);
    let winner1 = matcher1.match_sni(&key, "anything.example.com").unwrap();

    let matcher2 = GatewayTlsMatcher::new();
    matcher2.rebuild_from_gateways(&[gw_b, gw_a]);
    let winner2 = matcher2.match_sni(&key, "anything.example.com").unwrap();

    assert_eq!(winner1.gateway_name, winner2.gateway_name);
    assert_eq!(winner1.gateway_name, "alpha-gw");
    assert_eq!(winner1.certificate_refs[0].name, "alpha-cert");
}

#[test]
fn test_programmed_false_does_not_shadow_valid_listener() {
    use edgion_resources::resources::common::Condition;
    use edgion_resources::resources::gateway::{GatewayStatus, ListenerStatus};

    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    // Invalid gateway: Programmed=False, catch-all (no hostname), valid cert
    let invalid_tls = make_tls_config("invalid-cert", Some("default"));
    let invalid_listener = create_test_listener("https", None, Some(invalid_tls));
    let mut invalid_gw = create_test_gateway("invalid-gw", "default", vec![invalid_listener]);
    invalid_gw.status = Some(GatewayStatus {
        addresses: None,
        conditions: None,
        listeners: Some(vec![ListenerStatus {
            name: "https".to_string(),
            supported_kinds: vec![],
            attached_routes: 0,
            conditions: vec![Condition {
                last_transition_time: "2024-01-01T00:00:00Z".to_string(),
                message: String::new(),
                observed_generation: None,
                reason: "NoValidCACertificate".to_string(),
                status: "False".to_string(),
                type_: "Programmed".to_string(),
            }],
        }]),
    });

    // Valid gateway: Programmed=True, same port, specific hostname
    let valid_tls = make_tls_config("valid-cert", Some("default"));
    let valid_listener = create_test_listener("https", Some("api.example.com"), Some(valid_tls));
    let valid_gw = create_test_gateway("valid-gw", "default", vec![valid_listener]);

    matcher.rebuild_from_gateways(&[invalid_gw, valid_gw]);

    // Valid listener should still match
    let result = matcher.match_sni(&key, "api.example.com");
    assert!(result.is_some(), "valid listener should still be registered");
    assert_eq!(result.unwrap().gateway_name, "valid-gw");

    // Invalid listener's cert should NOT appear as catch-all fallback
    let fallback = matcher.match_sni(&key, "unknown.example.com");
    assert!(
        fallback.is_none() || fallback.as_ref().map(|e| e.gateway_name.as_str()) == Some("valid-gw"),
        "catch-all should not use the invalid gateway's cert"
    );
}
