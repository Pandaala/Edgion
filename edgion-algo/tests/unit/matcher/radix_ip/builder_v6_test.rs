use super::*;

#[test]
fn test_new_builder() {
    let builder = IpV6RadixBuilder::new();
    assert!(builder.is_empty());
}

#[test]
fn test_insert_single_rule() {
    let mut builder = IpV6RadixBuilder::new();
    assert!(builder.insert("2001:db8::/32", 1).is_ok());
    assert!(!builder.is_empty());
}

#[test]
fn test_insert_multiple_rules() {
    let mut builder = IpV6RadixBuilder::new();
    assert!(builder.insert("2001:db8::/32", 1).is_ok());
    assert!(builder.insert("fe80::/10", 2).is_ok());
    assert!(builder.insert("::1/128", 1).is_ok());
}

#[test]
fn test_reject_ipv4() {
    let mut builder = IpV6RadixBuilder::new();
    let result = builder.insert("192.168.1.0/24", 1);
    assert!(result.is_err());
}

#[test]
fn test_invalid_cidr() {
    let mut builder = IpV6RadixBuilder::new();
    assert!(builder.insert("2001:db8::", 1).is_err());
    assert!(builder.insert("invalid/32", 1).is_err());
}

#[test]
fn test_freeze_empty_tree() {
    let result = IpV6RadixBuilder::new().freeze();
    assert!(matches!(result, Err(IpRadixError::EmptyTree)));
}

#[test]
fn duplicate_cidr_detected() {
    let mut builder = IpV6RadixBuilder::new();
    builder.insert("2001:db8::/32", 1).unwrap();
    let err = builder.insert("2001:db8::/32", 2).unwrap_err();
    assert!(matches!(err, IpRadixError::DuplicateCidr { .. }));
}

#[test]
fn duplicate_cidr_same_label_is_idempotent() {
    let mut builder = IpV6RadixBuilder::new();
    builder.insert("2001:db8::/32", 1).unwrap();
    assert!(builder.insert("2001:db8::/32", 1).is_ok());
}
