use super::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn addr(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)
}

fn key(port: u16) -> ConflictKey {
    make_conflict_key(addr(port))
}

#[test]
fn conflict_messages_are_stable_across_registration_order() {
    let manager = ListenerPortManager::new();
    let mut listeners = vec![
        ("z-http".to_string(), key(80), None),
        ("a-tcp".to_string(), key(80), None),
        ("m-tls".to_string(), key(80), None),
    ];
    let expected = HashMap::from([
        (
            "a-tcp".to_string(),
            "Socket 0.0.0.0:80: conflicts with: m-tls, z-http".to_string(),
        ),
        (
            "m-tls".to_string(),
            "Socket 0.0.0.0:80: conflicts with: a-tcp, z-http".to_string(),
        ),
        (
            "z-http".to_string(),
            "Socket 0.0.0.0:80: conflicts with: a-tcp, m-tls".to_string(),
        ),
    ]);
    for round in 0..12 {
        listeners.rotate_left(1);
        if round % 2 == 0 {
            listeners.reverse();
        }
        manager.register_gateway("default/gateway", &listeners);
        assert_eq!(manager.detect_conflicts("default/gateway"), expected);
    }
    manager.unregister_gateway("default/gateway");
    assert_eq!(manager.stats(), (0, 0));
}

#[test]
fn test_make_conflict_key() {
    // All protocols on the same (ip, port) produce the same conflict key
    assert_eq!(make_conflict_key(addr(80)), "0.0.0.0:80");
    assert_eq!(make_conflict_key(addr(443)), "0.0.0.0:443");

    // Different ports produce different keys
    assert_ne!(make_conflict_key(addr(80)), make_conflict_key(addr(443)));

    // Custom IP address
    let custom = SocketAddr::new("192.168.1.1".parse::<IpAddr>().unwrap(), 8080);
    assert_eq!(make_conflict_key(custom), "192.168.1.1:8080");
}

#[test]
fn test_hostnames_conflict_fn() {
    // Both wildcard (None) → conflict
    assert!(hostnames_conflict(None, None));

    // Wildcard vs concrete → no conflict (overlapping but not identical)
    assert!(!hostnames_conflict(None, Some("example.com")));
    assert!(!hostnames_conflict(Some("example.com"), None));

    // Exact match → conflict
    assert!(hostnames_conflict(Some("example.com"), Some("example.com")));

    // Different concrete hostnames → no conflict
    assert!(!hostnames_conflict(Some("api.example.com"), Some("web.example.com")));

    // Wildcard pattern vs concrete → no conflict (overlapping but not identical)
    assert!(!hostnames_conflict(Some("*.example.com"), Some("foo.example.com")));
    assert!(!hostnames_conflict(Some("foo.example.com"), Some("*.example.com")));

    // Different wildcard patterns → no conflict
    assert!(!hostnames_conflict(Some("*.example.com"), Some("*.bar.example.com")));
    assert!(!hostnames_conflict(Some("*.foo.com"), Some("*.bar.com")));

    // Same wildcard pattern → conflict
    assert!(hostnames_conflict(Some("*.example.com"), Some("*.example.com")));
}

#[test]
fn test_register_and_unregister() {
    let manager = ListenerPortManager::new();

    manager.register_gateway(
        "default/gateway-a",
        &[
            ("http".to_string(), key(80), None),
            ("https".to_string(), key(443), Some("example.com".to_string())),
        ],
    );

    manager.register_gateway("default/gateway-b", &[("http".to_string(), key(80), None)]);

    let (sockets, gateways) = manager.stats();
    assert_eq!(gateways, 2);
    assert_eq!(sockets, 2); // "0.0.0.0:80" and "0.0.0.0:443"

    manager.unregister_gateway("default/gateway-a");

    let (sockets, gateways) = manager.stats();
    assert_eq!(gateways, 1);
    assert_eq!(sockets, 1);
}

#[test]
fn test_conflict_same_socket_same_hostname() {
    let manager = ListenerPortManager::new();

    manager.register_gateway(
        "default/gw",
        &[
            ("http-1".to_string(), key(80), None),
            ("http-2".to_string(), key(80), None),
        ],
    );

    let conflicts = manager.detect_conflicts("default/gw");
    assert_eq!(conflicts.len(), 2);
    assert!(conflicts.contains_key("http-1"));
    assert!(conflicts.contains_key("http-2"));
}

#[test]
fn test_no_conflict_wildcard_vs_specific() {
    let manager = ListenerPortManager::new();

    // Wildcard (None) vs concrete hostname — overlapping but not identical, no conflict
    manager.register_gateway(
        "default/gw",
        &[
            ("catchall".to_string(), key(443), None),
            ("specific".to_string(), key(443), Some("api.example.com".to_string())),
        ],
    );

    assert!(manager.detect_conflicts("default/gw").is_empty());
}

#[test]
fn test_no_conflict_wildcard_pattern_vs_concrete() {
    let manager = ListenerPortManager::new();

    // *.example.com vs foo.example.com — overlapping but not identical, no conflict
    manager.register_gateway(
        "default/gw",
        &[
            ("wild".to_string(), key(443), Some("*.example.com".to_string())),
            ("concrete".to_string(), key(443), Some("foo.example.com".to_string())),
        ],
    );

    assert!(manager.detect_conflicts("default/gw").is_empty());
}

#[test]
fn test_conflict_same_wildcard_pattern() {
    let manager = ListenerPortManager::new();

    // Two listeners with identical wildcard pattern → conflict
    manager.register_gateway(
        "default/gw",
        &[
            ("wild-1".to_string(), key(443), Some("*.example.com".to_string())),
            ("wild-2".to_string(), key(443), Some("*.example.com".to_string())),
        ],
    );

    let conflicts = manager.detect_conflicts("default/gw");
    assert_eq!(conflicts.len(), 2);
}

#[test]
fn test_no_conflict_different_hostnames() {
    let manager = ListenerPortManager::new();

    // Different concrete hostnames on same socket address — no conflict
    manager.register_gateway(
        "default/gw",
        &[
            ("api".to_string(), key(443), Some("api.example.com".to_string())),
            ("web".to_string(), key(443), Some("web.example.com".to_string())),
        ],
    );

    assert!(manager.detect_conflicts("default/gw").is_empty());
}

#[test]
fn test_no_conflict_cross_gateway() {
    let manager = ListenerPortManager::new();

    manager.register_gateway("default/gw-a", &[("https".to_string(), key(443), None)]);
    manager.register_gateway("default/gw-b", &[("https".to_string(), key(443), None)]);

    assert!(manager.detect_conflicts("default/gw-a").is_empty());
    assert!(manager.detect_conflicts("default/gw-b").is_empty());
}

#[test]
fn test_no_conflict_different_ports() {
    let manager = ListenerPortManager::new();

    manager.register_gateway(
        "default/gw",
        &[
            ("http".to_string(), key(80), None),
            ("https".to_string(), key(443), None),
        ],
    );

    assert!(manager.detect_conflicts("default/gw").is_empty());
}

#[test]
fn test_conflict_tcp_and_http_same_socket() {
    let manager = ListenerPortManager::new();

    // TCP and HTTP on the same (ip, port) share the same OS socket → same conflict key → conflict
    manager.register_gateway(
        "default/gw",
        &[
            ("tcp-80".to_string(), key(80), None),
            ("http-80".to_string(), key(80), None),
        ],
    );

    let conflicts = manager.detect_conflicts("default/gw");
    assert_eq!(conflicts.len(), 2);
    assert!(conflicts.contains_key("tcp-80"));
    assert!(conflicts.contains_key("http-80"));
}

#[test]
fn test_re_register_clears_old() {
    let manager = ListenerPortManager::new();

    manager.register_gateway("default/gw", &[("http".to_string(), key(80), None)]);
    manager.register_gateway("default/gw", &[]);

    let (sockets, gateways) = manager.stats();
    assert_eq!(gateways, 0);
    assert_eq!(sockets, 0);
}

#[test]
fn test_clear() {
    let manager = ListenerPortManager::new();
    manager.register_gateway("default/gw", &[("http".to_string(), key(80), None)]);
    manager.clear();
    let (sockets, gateways) = manager.stats();
    assert_eq!(sockets, 0);
    assert_eq!(gateways, 0);
}

#[test]
fn test_no_conflict_wildcard_and_two_concretes() {
    let manager = ListenerPortManager::new();

    // Wildcard + two concretes — all different hostnames, no conflicts
    manager.register_gateway(
        "default/gw",
        &[
            ("catchall".to_string(), key(443), None),
            ("api".to_string(), key(443), Some("api.example.com".to_string())),
            ("web".to_string(), key(443), Some("web.example.com".to_string())),
        ],
    );

    assert!(manager.detect_conflicts("default/gw").is_empty());
}

#[test]
fn test_conflict_two_wildcards_and_concrete() {
    let manager = ListenerPortManager::new();

    // Two wildcard (None) listeners → conflict with each other; concrete is fine
    manager.register_gateway(
        "default/gw",
        &[
            ("catchall-1".to_string(), key(443), None),
            ("catchall-2".to_string(), key(443), None),
            ("api".to_string(), key(443), Some("api.example.com".to_string())),
        ],
    );

    let conflicts = manager.detect_conflicts("default/gw");
    assert_eq!(conflicts.len(), 2);
    assert!(conflicts.contains_key("catchall-1"));
    assert!(conflicts.contains_key("catchall-2"));
    assert!(!conflicts.contains_key("api"));
}
