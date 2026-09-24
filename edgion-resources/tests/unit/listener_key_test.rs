use super::*;
use std::net::{IpAddr, Ipv4Addr};

fn make_addr(ip: &str, port: u16) -> SocketAddr {
    SocketAddr::new(ip.parse::<IpAddr>().unwrap(), port)
}

#[test]
fn test_new_and_accessors() {
    let addr = make_addr("0.0.0.0", 80);
    let key = ListenerKey::new(addr, "HTTP");
    assert_eq!(key.protocol(), "HTTP");
}

#[test]
fn test_protocol_uppercased() {
    let addr = make_addr("127.0.0.1", 443);
    let key = ListenerKey::new(addr, "https");
    assert_eq!(key.protocol(), "HTTPS");
}

#[test]
fn test_display() {
    let addr = make_addr("0.0.0.0", 8080);
    let key = ListenerKey::new(addr, "GRPC");
    assert_eq!(format!("{}", key), "0.0.0.0:8080/GRPC");
}

#[test]
fn test_as_ref() {
    let addr = make_addr("0.0.0.0", 80);
    let key = ListenerKey::new(addr, "HTTP");
    let s: &str = key.as_ref();
    assert_eq!(s, "0.0.0.0:80/HTTP");
}

#[test]
fn test_resolve_listener_addr_no_addresses() {
    let addr = resolve_listener_addr(None, 80);
    assert_eq!(addr, SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 80));
}

#[test]
fn test_resolve_listener_addr_with_ip() {
    let addrs = vec![GatewayAddress {
        address_type: None,
        value: Some("192.168.1.1".to_string()),
    }];
    let addr = resolve_listener_addr(Some(&addrs), 8080);
    assert_eq!(addr.to_string(), "192.168.1.1:8080");
}

#[test]
fn test_resolve_listener_addr_with_unparseable_value() {
    let addrs = vec![GatewayAddress {
        address_type: Some("Hostname".to_string()),
        value: Some("my-gateway.example.com".to_string()),
    }];
    let addr = resolve_listener_addr(Some(&addrs), 443);
    assert_eq!(addr, SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443));
}
