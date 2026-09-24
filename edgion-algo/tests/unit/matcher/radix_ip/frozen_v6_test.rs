use super::*;
use crate::matcher::radix_ip::builder_v6::IpV6RadixBuilder;

#[test]
fn test_basic_match() {
    let mut builder = IpV6RadixBuilder::new();
    builder.insert("2001:db8::/32", 1).unwrap();
    let tree = builder.freeze().unwrap();

    let matching = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
    let non_matching = Ipv6Addr::new(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1);
    assert_eq!(tree.match_ipv6(matching), Some(1));
    assert_eq!(tree.match_ipv6(non_matching), None);
}

#[test]
fn test_longest_prefix_match() {
    let mut builder = IpV6RadixBuilder::new();
    builder.insert("2001:db8::/32", 1).unwrap();
    builder.insert("2001:db8:1::/48", 2).unwrap();
    builder.insert("2001:db8:1:2::/64", 1).unwrap();
    let tree = builder.freeze().unwrap();

    let prefix_64 = Ipv6Addr::new(0x2001, 0xdb8, 1, 2, 0, 0, 0, 1);
    let prefix_48 = Ipv6Addr::new(0x2001, 0xdb8, 1, 3, 0, 0, 0, 1);
    let prefix_32 = Ipv6Addr::new(0x2001, 0xdb8, 2, 0, 0, 0, 0, 1);
    let no_match = Ipv6Addr::new(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1);
    assert_eq!(tree.match_ipv6(prefix_64), Some(1));
    assert_eq!(tree.match_ipv6(prefix_48), Some(2));
    assert_eq!(tree.match_ipv6(prefix_32), Some(1));
    assert_eq!(tree.match_ipv6(no_match), None);
}

#[test]
fn test_host_route() {
    let mut builder = IpV6RadixBuilder::new();
    builder.insert("2001:db8::1/128", 2).unwrap();
    let tree = builder.freeze().unwrap();

    let exact = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
    let neighbor = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2);
    assert_eq!(tree.match_ipv6(exact), Some(2));
    assert_eq!(tree.match_ipv6(neighbor), None);
}

#[test]
fn test_default_route() {
    let mut builder = IpV6RadixBuilder::new();
    builder.insert("::/0", 1).unwrap();
    let tree = builder.freeze().unwrap();

    let first = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
    let last = Ipv6Addr::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff);
    assert_eq!(tree.match_ipv6(first), Some(1));
    assert_eq!(tree.match_ipv6(last), Some(1));
}
