use super::*;

#[tokio::test]
async fn empty_servers_falls_back_to_system() {
    // No servers configured ⇒ behave exactly like the system resolver.
    let addrs = resolve_standard(&[], "localhost", crate::dns::DnsQueryPolicy::Native)
        .await
        .expect("system fallback resolves");
    assert!(addrs.iter().any(|ip| ip.is_loopback()));
}

#[test]
fn parses_server_with_and_without_port() {
    assert_eq!(
        parse_server("1.1.1.1").unwrap(),
        "1.1.1.1:53".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(
        parse_server("1.1.1.1:5353").unwrap(),
        "1.1.1.1:5353".parse::<SocketAddr>().unwrap()
    );
    assert!(parse_server("not an ip").is_none());
}

#[test]
fn parses_bracketed_ipv6_without_port() {
    assert_eq!(
        parse_server("[::1]").unwrap(),
        SocketAddr::new("::1".parse::<IpAddr>().unwrap(), 53)
    );
    assert_eq!(
        parse_server("[2001:db8::1]").unwrap(),
        SocketAddr::new("2001:db8::1".parse::<IpAddr>().unwrap(), 53)
    );
}

// DR-1: each server must keep its own configured port instead of every
// server inheriting the first server's port.
#[test]
fn parse_servers_preserves_per_server_ports() {
    let servers = vec!["1.1.1.1:5353".to_string(), "8.8.8.8:9999".to_string()];
    let addrs = parse_servers(&servers);
    assert_eq!(addrs.len(), 2);
    assert!(addrs.contains(&"1.1.1.1:5353".parse::<SocketAddr>().unwrap()));
    assert!(addrs.contains(&"8.8.8.8:9999".parse::<SocketAddr>().unwrap()));
}
