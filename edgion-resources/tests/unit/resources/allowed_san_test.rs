use super::*;

#[test]
fn deserializes_compact_dns_exact() {
    let entry: AllowedSan = yaml_serde::from_str("type: DNS\nvalue: client1.example.com\n").unwrap();
    assert_eq!(entry.san_type, AllowedSanType::Dns);
    assert_eq!(entry.match_kind, AllowedSanMatch::Exact);
    assert!(!entry.ignore_case);
    assert!(entry.oid.is_none());
    assert!(allowed_san_config_error(&entry).is_none());
}

#[test]
fn rejects_unknown_field() {
    let err = yaml_serde::from_str::<AllowedSan>("type: DNS\nvalue: a.example.com\nextra: 1\n").unwrap_err();
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

    let empty_oid: AllowedSan = yaml_serde::from_str("type: DNS\nvalue: client.example.com\noid: \"\"\n").unwrap();
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

#[test]
fn regex_ignore_case_defaults_and_inline_overrides() {
    for (pattern, ignore_case, presented, expected) in [
        (r"^spiffe://cluster$", false, "SPIFFE://CLUSTER", false),
        (r"^spiffe://cluster$", true, "SPIFFE://CLUSTER", true),
        (r"(?i)^spiffe://cluster$", false, "SPIFFE://CLUSTER", true),
        (r"(?-i)^spiffe://cluster$", true, "SPIFFE://CLUSTER", false),
        (r"^spiffe://(?-i:cluster)$", true, "SPIFFE://cluster", true),
        (r"^spiffe://(?-i:cluster)$", true, "SPIFFE://CLUSTER", false),
        (r"^é$", true, "É", true),
        (r"^é$", false, "É", false),
    ] {
        let mut entry = AllowedSan::exact(AllowedSanType::Uri, pattern);
        entry.match_kind = AllowedSanMatch::RegularExpression;
        entry.ignore_case = ignore_case;
        assert!(allowed_san_config_error(&entry).is_none());
        entry.compile().unwrap();
        assert_eq!(
            entry.compiled_regex.as_ref().unwrap().is_match(presented),
            expected,
            "pattern={pattern}, ignoreCase={ignore_case}, presented={presented}"
        );
    }
}

#[test]
fn regex_ignore_case_preserves_admission_and_runtime_bounds() {
    for ignore_case in [false, true] {
        for pattern in [
            "(".to_string(),
            "a".repeat(ALLOWED_SAN_REGEX_MAX_LEN + 1),
            r"\w{10000}".to_string(),
        ] {
            let mut entry = AllowedSan::exact(AllowedSanType::Uri, pattern);
            entry.match_kind = AllowedSanMatch::RegularExpression;
            entry.ignore_case = ignore_case;
            let error = allowed_san_config_error(&entry).expect("invalid or oversized regex must be rejected");
            assert_eq!(entry.compile().unwrap_err(), error);
            assert!(entry.compiled_regex.is_none());
        }
    }
}

#[test]
fn regex_case_folding_expansion_is_rejected_during_admission() {
    let mut entry = AllowedSan::exact(AllowedSanType::Uri, r"[a-z]{5000}");
    entry.match_kind = AllowedSanMatch::RegularExpression;
    assert!(allowed_san_config_error(&entry).is_none());
    assert!(entry.compile().is_ok());

    // Unicode case folding expands this small source beyond the program cap.
    // Admission must use the same flag as Gateway reconstruction.
    entry.ignore_case = true;
    entry.compiled_regex = None;
    let error = allowed_san_config_error(&entry).expect("case-folded program must exceed the limit");
    assert!(error.contains("compiled limit"));
    assert_eq!(entry.compile().unwrap_err(), error);
    assert!(entry.compiled_regex.is_none());
}
