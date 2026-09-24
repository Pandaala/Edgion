use super::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr as StdSocketAddr};

fn inet(ip: IpAddr, port: u16) -> SocketAddr {
    SocketAddr::Inet(StdSocketAddr::new(ip, port))
}

#[test]
fn ordinary_connection_has_equal_identities() {
    let peer = inet(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)), 1234);
    let got = resolve_transport_client_addrs(Some(&peer), Some(&peer), "");
    assert_eq!(got.direct_peer_ip, "192.0.2.10");
    assert_eq!(got.remote_ip, "192.0.2.10");
    assert_eq!(got.direct_peer_port, 1234);
    assert_eq!(got.remote_port, 1234);
}

#[test]
fn proxy_connection_preserves_distinct_raw_and_effective_peers() {
    let client = inet(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8)), 4321);
    let proxy = inet(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 20)), 2345);
    let got = resolve_transport_client_addrs(Some(&client), Some(&proxy), "");
    assert_eq!(got.direct_peer_ip, "192.0.2.20");
    assert_eq!(got.remote_ip, "203.0.113.8");
    assert_eq!(got.direct_peer_port, 2345);
    assert_eq!(got.remote_port, 4321);
}

#[test]
fn missing_raw_peer_falls_back_to_effective_and_addresses_are_canonical() {
    let mapped = inet(IpAddr::V6(Ipv4Addr::new(198, 51, 100, 7).to_ipv6_mapped()), 3456);
    let got = resolve_transport_client_addrs(Some(&mapped), None, "unknown");
    assert_eq!(got.direct_peer_ip, "198.51.100.7");
    assert_eq!(got.remote_ip, "198.51.100.7");
    assert_eq!(got.direct_peer_port, 3456);
}

#[test]
fn ipv6_and_missing_effective_are_stable() {
    let ipv6 = inet(IpAddr::V6("2001:db8::1".parse().unwrap()), 4567);
    let got = resolve_transport_client_addrs(Some(&ipv6), Some(&ipv6), "unknown");
    assert_eq!(got.remote_ip, "2001:db8::1");

    let missing = resolve_transport_client_addrs(None, None, "unknown");
    assert_eq!(missing.remote_ip, "unknown");
    assert_eq!(missing.direct_peer_ip, "unknown");
    assert_eq!(missing.remote_port, 0);
}
