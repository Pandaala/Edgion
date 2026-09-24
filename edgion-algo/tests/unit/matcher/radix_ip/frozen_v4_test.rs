use super::*;
use crate::matcher::radix_ip::builder_v4::IpV4RadixBuilder;

#[test]
fn test_basic_match() {
    let mut builder = IpV4RadixBuilder::new();
    builder.insert("192.168.1.0/24", 1).unwrap();
    let tree = builder.freeze().unwrap();

    assert_eq!(tree.match_ipv4(Ipv4Addr::new(192, 168, 1, 50)), Some(1));
    assert_eq!(tree.match_ipv4(Ipv4Addr::new(192, 168, 2, 1)), None);
}

#[test]
fn test_longest_prefix_match() {
    let mut builder = IpV4RadixBuilder::new();
    builder.insert("10.0.0.0/8", 1).unwrap();
    builder.insert("10.1.0.0/16", 2).unwrap();
    builder.insert("10.1.1.0/24", 1).unwrap();
    let tree = builder.freeze().unwrap();

    assert_eq!(tree.match_ipv4(Ipv4Addr::new(10, 1, 1, 50)), Some(1));
    assert_eq!(tree.match_ipv4(Ipv4Addr::new(10, 1, 2, 50)), Some(2));
    assert_eq!(tree.match_ipv4(Ipv4Addr::new(10, 2, 1, 50)), Some(1));
    assert_eq!(tree.match_ipv4(Ipv4Addr::new(11, 0, 0, 1)), None);
}

#[test]
fn test_host_route() {
    let mut builder = IpV4RadixBuilder::new();
    builder.insert("192.168.1.100/32", 2).unwrap();
    let tree = builder.freeze().unwrap();

    assert_eq!(tree.match_ipv4(Ipv4Addr::new(192, 168, 1, 100)), Some(2));
    assert_eq!(tree.match_ipv4(Ipv4Addr::new(192, 168, 1, 101)), None);
}

#[test]
fn test_default_route() {
    let mut builder = IpV4RadixBuilder::new();
    builder.insert("0.0.0.0/0", 1).unwrap();
    let tree = builder.freeze().unwrap();

    assert_eq!(tree.match_ipv4(Ipv4Addr::new(1, 2, 3, 4)), Some(1));
    assert_eq!(tree.match_ipv4(Ipv4Addr::new(255, 255, 255, 255)), Some(1));
}
