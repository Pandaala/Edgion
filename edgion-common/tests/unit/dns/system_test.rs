use super::*;

#[tokio::test]
async fn resolves_localhost_to_loopback() {
    let addrs = resolve_system("localhost").await.expect("localhost resolves");
    assert!(
        addrs.iter().any(|ip| ip.is_loopback()),
        "expected a loopback, got {addrs:?}"
    );
}

#[tokio::test]
async fn empty_host_errors() {
    assert!(resolve_system("").await.is_err());
}

#[tokio::test]
async fn resolves_ipv6_literal_directly() {
    let addrs = resolve_system("::1").await.expect("ipv6 literal resolves");
    assert!(addrs.contains(&"::1".parse::<IpAddr>().unwrap()), "got {addrs:?}");
}

#[tokio::test]
async fn resolves_ipv4_literal_directly() {
    let addrs = resolve_system("127.0.0.1").await.expect("ipv4 literal resolves");
    assert!(addrs.contains(&"127.0.0.1".parse::<IpAddr>().unwrap()), "got {addrs:?}");
}
