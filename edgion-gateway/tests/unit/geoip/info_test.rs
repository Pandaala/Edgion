use super::*;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[test]
fn ipv4_mapped_ipv6_normalizes_to_ipv4() {
    let mapped = IpAddr::V6("::ffff:203.0.113.5".parse::<Ipv6Addr>().unwrap());
    let plain = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5));
    assert_eq!(normalize_addr(mapped), plain);
    assert_eq!(normalize_addr(plain), plain);
}

#[test]
fn genuine_ipv6_is_left_alone() {
    let v6: IpAddr = "2001:db8::1".parse().unwrap();
    assert_eq!(normalize_addr(v6), v6);
}
