//! Unit tests for `config.rs` (see `super`).
use super::*;
use crate::types::resources::gateway::{GatewaySpec, Listener};
use kube::api::ObjectMeta;

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
        },
        status: None,
    }
}

fn create_test_listener(name: &str, port: i32, hostname: Option<&str>) -> Listener {
    Listener {
        name: name.to_string(),
        hostname: hostname.map(|s| s.to_string()),
        port,
        protocol: "HTTPS".to_string(),
        tls: None,
        allowed_routes: None,
    }
}

#[test]
fn test_gateway_info() {
    let info = GatewayInfo::new(
        Some("default".to_string()),
        "my-gateway".to_string(),
        Some("https".to_string()),
        None,
        None,
        None,
    );
    assert_eq!(info.gateway_key(), "default/my-gateway");
    assert_eq!(info.namespace_str(), "default");
    assert_eq!(info.listener_name, Some("https".to_string()));

    let info_no_ns = GatewayInfo::new(None, "my-gateway".to_string(), None, None, None, None);
    assert_eq!(info_no_ns.gateway_key(), "my-gateway");
    assert_eq!(info_no_ns.namespace_str(), "");
    assert_eq!(info_no_ns.listener_name, None);
}

#[test]
fn test_parse_gateway_to_config() {
    let listeners = vec![
        create_test_listener("http", 80, Some("example.com")),
        create_test_listener("https", 443, Some("*.example.com")),
        create_test_listener("admin", 8443, None),
    ];
    let gateway = create_test_gateway("test-gw", "default", listeners);

    let config = parse_gateway_to_config(&gateway);

    // Check listener_map
    assert!(config.has_listener("http"));
    assert!(config.has_listener("https"));
    assert!(config.has_listener("admin"));
    assert!(!config.has_listener("unknown"));

    // Check host_map (exact)
    assert!(config.has_host("example.com"));

    // Check wildcard
    assert!(config.has_host("api.example.com"));
    assert!(config.has_host("www.example.com"));
}

#[test]
fn test_has_more_specific_listener_catchall_vs_exact() {
    let listeners = vec![
        create_test_listener("catch-all", 80, None),
        create_test_listener("exact", 80, Some("abc.foo.example.com")),
    ];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    // catch-all should be blocked because exact listener claims this hostname
    assert!(config.has_more_specific_listener("abc.foo.example.com", None, 80));
    // catch-all should NOT be blocked for an unrelated hostname
    assert!(!config.has_more_specific_listener("bar.com", None, 80));
}

#[test]
fn test_has_more_specific_listener_catchall_vs_wildcard() {
    let listeners = vec![
        create_test_listener("catch-all", 80, None),
        create_test_listener("wildcard", 80, Some("*.example.com")),
    ];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    // catch-all blocked: *.example.com matches foo.example.com
    assert!(config.has_more_specific_listener("foo.example.com", None, 80));
    // catch-all NOT blocked: *.example.com doesn't match bar.com
    assert!(!config.has_more_specific_listener("bar.com", None, 80));
}

#[test]
fn test_has_more_specific_listener_catchall_no_others() {
    let listeners = vec![create_test_listener("catch-all", 80, None)];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    assert!(!config.has_more_specific_listener("anything.com", None, 80));
}

#[test]
fn test_has_more_specific_listener_wildcard_vs_exact() {
    let listeners = vec![
        create_test_listener("wildcard", 80, Some("*.example.com")),
        create_test_listener("exact", 80, Some("abc.foo.example.com")),
    ];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    // wildcard blocked: exact listener claims abc.foo.example.com
    assert!(config.has_more_specific_listener("abc.foo.example.com", Some("*.example.com"), 80));
    // wildcard NOT blocked: no exact listener for bar.example.com
    assert!(!config.has_more_specific_listener("bar.example.com", Some("*.example.com"), 80));
}

#[test]
fn test_has_more_specific_listener_wildcard_vs_more_specific_wildcard() {
    let listeners = vec![
        create_test_listener("broad", 80, Some("*.example.com")),
        create_test_listener("specific", 80, Some("*.foo.example.com")),
    ];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    // *.example.com blocked for abc.foo.example.com because *.foo.example.com is more specific
    assert!(config.has_more_specific_listener("abc.foo.example.com", Some("*.example.com"), 80));
    // *.foo.example.com NOT blocked — it IS the most specific wildcard
    assert!(!config.has_more_specific_listener("abc.foo.example.com", Some("*.foo.example.com"), 80));
}

#[test]
fn test_has_more_specific_listener_wildcard_no_more_specific() {
    let listeners = vec![create_test_listener("wildcard", 80, Some("*.example.com"))];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    assert!(!config.has_more_specific_listener("foo.example.com", Some("*.example.com"), 80));
}

#[test]
fn test_has_more_specific_listener_exact_always_false() {
    let listeners = vec![
        create_test_listener("exact", 80, Some("foo.example.com")),
        create_test_listener("wildcard", 80, Some("*.example.com")),
        create_test_listener("catch-all", 80, None),
    ];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    // Exact listener is never blocked — nothing is more specific
    assert!(!config.has_more_specific_listener("foo.example.com", Some("foo.example.com"), 80));
}

#[test]
fn test_has_more_specific_listener_full_isolation() {
    // Mimics GatewayHTTPListenerIsolation conformance scenario:
    //   listeners: empty-hostname, *.example.com, *.foo.example.com, abc.foo.example.com
    let listeners = vec![
        create_test_listener("empty", 80, None),
        create_test_listener("wildcard-example", 80, Some("*.example.com")),
        create_test_listener("wildcard-foo", 80, Some("*.foo.example.com")),
        create_test_listener("exact-abc", 80, Some("abc.foo.example.com")),
    ];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    let host = "abc.foo.example.com";
    // empty blocked: exact listener claims it
    assert!(config.has_more_specific_listener(host, None, 80));
    // *.example.com blocked: exact listener claims it
    assert!(config.has_more_specific_listener(host, Some("*.example.com"), 80));
    // *.foo.example.com blocked: exact listener claims it
    assert!(config.has_more_specific_listener(host, Some("*.foo.example.com"), 80));
    // abc.foo.example.com NOT blocked
    assert!(!config.has_more_specific_listener(host, Some("abc.foo.example.com"), 80));

    let host2 = "bar.foo.example.com";
    // empty blocked: *.foo.example.com matches
    assert!(config.has_more_specific_listener(host2, None, 80));
    // *.example.com blocked: *.foo.example.com is more specific
    assert!(config.has_more_specific_listener(host2, Some("*.example.com"), 80));
    // *.foo.example.com NOT blocked — it IS the most specific
    assert!(!config.has_more_specific_listener(host2, Some("*.foo.example.com"), 80));

    let host3 = "bar.example.com";
    // empty blocked: *.example.com matches
    assert!(config.has_more_specific_listener(host3, None, 80));
    // *.example.com NOT blocked — most specific match for bar.example.com
    assert!(!config.has_more_specific_listener(host3, Some("*.example.com"), 80));

    let host4 = "bar.com";
    // empty NOT blocked: no other listener matches bar.com
    assert!(!config.has_more_specific_listener(host4, None, 80));
}

#[test]
fn test_has_more_specific_listener_cross_port_independent() {
    // Listeners on different ports should NOT trigger isolation.
    // HTTP catch-all on port 80, HTTPS wildcard on port 443.
    let listeners = vec![
        create_test_listener("http", 80, None),
        create_test_listener("https", 443, Some("*.example.com")),
    ];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    // Catch-all on port 80: the *.example.com listener is on port 443,
    // so it must NOT block catch-all on port 80.
    assert!(!config.has_more_specific_listener("foo.example.com", None, 80));

    // On port 443, *.example.com is the only listener, nothing blocks it
    assert!(!config.has_more_specific_listener("foo.example.com", Some("*.example.com"), 443));
}

#[test]
fn test_add_listener_silently_rejected_wildcard_still_in_listener_map() {
    // HashHost rejects wildcards whose stripped suffix is not a multi-label
    // domain (e.g. "*.localhost"). The listener must still appear in
    // listener_map so that routes targeting it via sectionName keep working;
    // only hostname-based lookups should miss.
    let listeners = vec![
        create_test_listener("internal", 80, Some("*.localhost")),
        create_test_listener("public", 80, Some("*.example.com")),
    ];
    let gw = create_test_gateway("gw", "default", listeners);
    let config = parse_gateway_to_config(&gw);

    // sectionName route: both listeners reachable
    assert!(config.has_listener("internal"));
    assert!(config.has_listener("public"));

    // Hostname-based: the rejected wildcard does NOT match
    assert!(!config.has_host("foo.localhost"));
    // The accepted wildcard still works (verifies the rejection didn't
    // poison the shared HashHost state)
    assert!(config.has_host("api.example.com"));
}

#[test]
fn test_store_update_and_remove() {
    let store = GatewayConfigStore::new();

    let gw = create_test_gateway(
        "test-gw",
        "default",
        vec![create_test_listener("https", 443, Some("example.com"))],
    );

    // Update
    store.update_gateway(&gw);
    assert!(store.has_listener("default", "test-gw", "https"));

    // Remove
    store.remove_gateway("default", "test-gw");
    assert!(!store.has_listener("default", "test-gw", "https"));
}
