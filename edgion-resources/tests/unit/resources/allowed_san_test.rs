use super::*;

#[test]
fn deserializes_compact_dns_exact() {
    let entry: AllowedSan = serde_yaml::from_str("type: DNS\nvalue: client1.example.com\n").unwrap();
    assert_eq!(entry.san_type, AllowedSanType::Dns);
    assert_eq!(entry.match_kind, AllowedSanMatch::Exact);
    assert!(!entry.ignore_case);
    assert!(entry.oid.is_none());
    assert!(allowed_san_config_error(&entry).is_none());
}

#[test]
fn rejects_unknown_field() {
    let err = serde_yaml::from_str::<AllowedSan>("type: DNS\nvalue: a.example.com\nextra: 1\n").unwrap_err();
    assert!(err.to_string().contains("unknown field"));
}

#[test]
fn other_name_requires_oid() {
    let entry = AllowedSan::exact(AllowedSanType::OtherName, "alice@corp.example.com");
    assert!(allowed_san_config_error(&entry).unwrap().contains("oid is required"));
}

#[test]
fn oid_forbidden_on_dns() {
    let mut entry = AllowedSan::exact(AllowedSanType::Dns, "client.example.com");
    entry.oid = Some("1.2.3".to_string());
    assert!(allowed_san_config_error(&entry).unwrap().contains("only valid"));

    let empty_oid: AllowedSan = serde_yaml::from_str("type: DNS\nvalue: client.example.com\noid: \"\"\n").unwrap();
    assert!(allowed_san_config_error(&empty_oid).unwrap().contains("only valid"));
}

#[test]
fn dns_exact_rejects_uppercase_and_nested_wildcard() {
    let upper = AllowedSan::exact(AllowedSanType::Dns, "Client.example.com");
    assert!(allowed_san_config_error(&upper).is_some());
    let nested = AllowedSan::exact(AllowedSanType::Dns, "*.*.example.com");
    assert!(allowed_san_config_error(&nested).is_some());
    let ok = AllowedSan::exact(AllowedSanType::Dns, "*.internal.example.com");
    assert!(allowed_san_config_error(&ok).is_none());
}

#[test]
fn ip_exact_rejects_garbage() {
    let entry = AllowedSan::exact(AllowedSanType::Ip, "not-an-ip");
    assert!(allowed_san_config_error(&entry).unwrap().contains("parseable"));
}

#[test]
fn regex_compile_and_size_cap() {
    let mut entry = AllowedSan::exact(AllowedSanType::Uri, "^spiffe://cluster\\.local/.*$");
    entry.match_kind = AllowedSanMatch::RegularExpression;
    assert!(allowed_san_config_error(&entry).is_none());
    entry.compile().unwrap();
    assert!(entry.compiled_regex.is_some());

    let too_long = AllowedSan {
        san_type: AllowedSanType::Dns,
        match_kind: AllowedSanMatch::RegularExpression,
        value: "a".repeat(ALLOWED_SAN_REGEX_MAX_LEN + 1),
        ignore_case: false,
        oid: None,
        compiled_regex: None,
    };
    assert!(allowed_san_config_error(&too_long).unwrap().contains("exceeds"));
}

#[test]
fn empty_value_rejected() {
    let entry = AllowedSan::exact(AllowedSanType::Uri, "  ");
    assert!(allowed_san_config_error(&entry).unwrap().contains("non-empty"));
}
