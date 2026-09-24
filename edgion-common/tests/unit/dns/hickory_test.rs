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
