use super::*;

#[test]
fn test_new_builder() {
    let builder = IpV4RadixBuilder::new();
    assert!(builder.is_empty());
}

#[test]
fn test_insert_single_rule() {
    let mut builder = IpV4RadixBuilder::new();
    assert!(builder.insert("192.168.1.0/24", 1).is_ok());
    assert!(!builder.is_empty());
}

#[test]
fn test_insert_multiple_rules() {
    let mut builder = IpV4RadixBuilder::new();
    assert!(builder.insert("10.0.0.0/8", 1).is_ok());
    assert!(builder.insert("192.168.0.0/16", 2).is_ok());
    assert!(builder.insert("172.16.0.0/12", 1).is_ok());
}

#[test]
fn test_reject_ipv6() {
    let mut builder = IpV4RadixBuilder::new();
    let result = builder.insert("2001:db8::/32", 1);
    assert!(result.is_err());
}

#[test]
fn test_invalid_cidr() {
    let mut builder = IpV4RadixBuilder::new();
    assert!(builder.insert("192.168.1.0", 1).is_err());
    assert!(builder.insert("invalid/24", 1).is_err());
}

#[test]
fn test_freeze_empty_tree() {
    let result = IpV4RadixBuilder::new().freeze();
    assert!(matches!(result, Err(IpRadixError::EmptyTree)));
}

#[test]
fn duplicate_cidr_detected() {
    let mut builder = IpV4RadixBuilder::new();
    builder.insert("10.0.0.0/24", 1).unwrap();
    let err = builder.insert("10.0.0.0/24", 2).unwrap_err();
    assert!(matches!(err, IpRadixError::DuplicateCidr { .. }));
}

#[test]
fn duplicate_cidr_same_label_is_idempotent() {
    let mut builder = IpV4RadixBuilder::new();
    builder.insert("10.0.0.0/24", 1).unwrap();
    assert!(builder.insert("10.0.0.0/24", 1).is_ok());
}
