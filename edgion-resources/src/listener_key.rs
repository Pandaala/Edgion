//! ListenerKey — physical listener identity type.
//!
//! Format: `"{addr}/{PROTOCOL}"`, e.g. `"0.0.0.0:80/HTTP"`.
//! Used as primary key for all per-listener data structures.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::gateway::GatewayAddress;

/// Physical listener identity: "{addr}/{protocol}", e.g. "0.0.0.0:80/HTTP"
/// Used as primary key for all per-listener data structures.
#[derive(Clone, Hash, Eq, PartialEq, Debug, Serialize, Deserialize, JsonSchema, Ord, PartialOrd)]
pub struct ListenerKey(pub String);

impl ListenerKey {
    /// Create a new `ListenerKey` from a socket address and protocol.
    ///
    /// The protocol is uppercased automatically, so `"http"` becomes `"HTTP"`.
    pub fn new(addr: SocketAddr, protocol: &str) -> Self {
        ListenerKey(format!("{}/{}", addr, protocol.to_uppercase()))
    }

    /// Returns the protocol portion of the key (everything after the last `/`).
    pub fn protocol(&self) -> &str {
        match self.0.rfind('/') {
            Some(pos) => &self.0[pos + 1..],
            None => "",
        }
    }
}

impl fmt::Display for ListenerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ListenerKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Resolve the bind address for a Gateway's listener.
///
/// Priority: `Gateway.spec.addresses[0]` → `"0.0.0.0"`.
///
/// If the first address is present and parseable as an `IpAddr`, it is used;
/// otherwise the wildcard IPv4 address `0.0.0.0` is used.
pub fn resolve_listener_addr(gateway_addresses: Option<&Vec<GatewayAddress>>, listener_port: i32) -> SocketAddr {
    let ip = gateway_addresses
        .and_then(|addrs| addrs.first())
        .and_then(|a| a.value.parse::<IpAddr>().ok())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    SocketAddr::new(ip, listener_port as u16)
}

#[cfg(test)]
mod tests {
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
            value: "192.168.1.1".to_string(),
        }];
        let addr = resolve_listener_addr(Some(&addrs), 8080);
        assert_eq!(addr.to_string(), "192.168.1.1:8080");
    }

    #[test]
    fn test_resolve_listener_addr_with_unparseable_value() {
        let addrs = vec![GatewayAddress {
            address_type: Some("Hostname".to_string()),
            value: "my-gateway.example.com".to_string(),
        }];
        let addr = resolve_listener_addr(Some(&addrs), 443);
        assert_eq!(addr, SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443));
    }
}
