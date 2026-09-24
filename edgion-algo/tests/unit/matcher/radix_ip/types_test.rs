use super::*;
use std::net::{Ipv4Addr, Ipv6Addr};

#[test]
fn test_parse_ipv4_cidr() {
    let cidr = IpCidr::parse("192.168.1.0/24").unwrap();
    match cidr {
        IpCidr::V4 { addr, prefix_len } => {
            assert_eq!(prefix_len, 24);
            assert_eq!(Ipv4Addr::from(addr).to_string(), "192.168.1.0");
        }
        _ => panic!("Expected IPv4 CIDR"),
    }
}

#[test]
fn test_parse_ipv6_cidr() {
    let cidr = IpCidr::parse("2001:db8::/32").unwrap();
    match cidr {
        IpCidr::V6 { addr, prefix_len } => {
            assert_eq!(prefix_len, 32);
            assert_eq!(Ipv6Addr::from(addr).to_string(), "2001:db8::");
        }
        _ => panic!("Expected IPv6 CIDR"),
    }
}

#[test]
fn test_normalize_ipv4() {
    // 192.168.1.100/24 should normalize to 192.168.1.0/24
    let cidr = IpCidr::parse("192.168.1.100/24").unwrap();
    match cidr {
        IpCidr::V4 { addr, prefix_len } => {
            assert_eq!(prefix_len, 24);
            assert_eq!(Ipv4Addr::from(addr).to_string(), "192.168.1.0");
        }
        _ => panic!("Expected IPv4 CIDR"),
    }
}

#[test]
fn test_invalid_cidr_format() {
    let result = IpCidr::parse("192.168.1.0");
    assert!(result.is_err());
}

#[test]
fn test_prefix_too_long() {
    let result = IpCidr::parse("192.168.1.0/33");
    assert!(matches!(
        result,
        Err(IpRadixError::PrefixTooLong {
            prefix_len: 33,
            max: 32
        })
    ));
}
