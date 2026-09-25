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
fn test_resolve_listener_addr_uses_local_wildcard() {
    let gateway: Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "namespace": "default"},
        "spec": {"gatewayClassName": "edgion", "addresses": [{"type": "IPAddress", "value": "198.51.100.7"}], "listeners": []}
    }))
    .unwrap();
    let addr = resolve_listener_addr(&gateway, 80);
    assert_eq!(addr, SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 80));
}

#[test]
fn test_local_bind_ip_distinguishes_same_port_gateways() {
    let make_gateway = |ip: &str| -> Gateway {
        serde_json::from_value(serde_json::json!({
            "metadata": {"name": "edge", "namespace": "default", "annotations": {"edgion.io/local-bind-ip": ip}},
            "spec": {"gatewayClassName": "edgion", "listeners": []}
        }))
        .unwrap()
    };
    let first = make_gateway("192.0.2.10");
    let second = make_gateway("192.0.2.11");
    assert_eq!(resolve_listener_addr(&first, 80), make_addr("192.0.2.10", 80));
    assert_ne!(resolve_listener_addr(&first, 80), resolve_listener_addr(&second, 80));
    assert_eq!(
        resolve_listener_addr(&make_gateway("::ffff:192.0.2.10"), 80),
        resolve_listener_addr(&first, 80)
    );
    assert!(gateway_local_bind_ip(&make_gateway("not-an-ip")).is_err());
}
