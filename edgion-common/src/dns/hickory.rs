//! Shared Hickory resolver construction for explicit DNS servers.

use std::net::SocketAddr;

use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolverConfig};

/// Build a resolver configuration that queries every server over UDP and TCP
/// while preserving each server's configured port.
pub(super) fn resolver_config(socket_addrs: &[SocketAddr]) -> ResolverConfig {
    let name_servers = socket_addrs
        .iter()
        .map(|addr| {
            let mut udp = ConnectionConfig::udp();
            udp.port = addr.port();
            let mut tcp = ConnectionConfig::tcp();
            tcp.port = addr.port();
            NameServerConfig::new(addr.ip(), true, vec![udp, tcp])
        })
        .collect();
    ResolverConfig::from_parts(None, vec![], name_servers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_resolver::config::ProtocolConfig;

    #[test]
    fn preserves_each_server_port_for_udp_and_tcp() {
        let addrs = [
            "192.0.2.1:5353".parse::<SocketAddr>().unwrap(),
            "[2001:db8::1]:9953".parse::<SocketAddr>().unwrap(),
        ];
        let config = resolver_config(&addrs);

        assert_eq!(config.name_servers.len(), 2);
        for (server, expected) in config.name_servers.iter().zip(addrs) {
            assert_eq!(server.ip, expected.ip());
            assert_eq!(server.connections.len(), 2);
            assert!(server.connections.iter().any(|connection| {
                connection.port == expected.port() && matches!(connection.protocol, ProtocolConfig::Udp)
            }));
            assert!(server.connections.iter().any(|connection| {
                connection.port == expected.port() && matches!(connection.protocol, ProtocolConfig::Tcp)
            }));
        }
    }
}
