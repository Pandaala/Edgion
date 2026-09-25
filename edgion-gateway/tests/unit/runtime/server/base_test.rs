use super::*;

#[test]
fn same_port_distinct_host_ips_are_independent() {
    let first = "192.0.2.10:8080".parse().unwrap();
    let second = "192.0.2.11:8080".parse().unwrap();
    let wildcard = "0.0.0.0:8080".parse().unwrap();
    assert!(!socket_bindings_overlap(first, false, second, false));
    assert!(socket_bindings_overlap(wildcard, false, first, false));
    assert!(socket_bindings_overlap(first, false, first, false));
    assert!(!socket_bindings_overlap(first, false, first, true));
    assert!(!socket_bindings_overlap(
        wildcard,
        false,
        "[::1]:8080".parse().unwrap(),
        false
    ));
    assert!(socket_bindings_overlap(
        "[::]:8080".parse().unwrap(),
        false,
        first,
        false
    ));
    assert!(socket_bindings_overlap(
        first,
        false,
        "[::ffff:192.0.2.10]:8080".parse().unwrap(),
        false
    ));
}
