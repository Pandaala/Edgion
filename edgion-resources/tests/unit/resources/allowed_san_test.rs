use super::*;

fn prepared_regex(entry: &AllowedSan) -> Option<&regex::Regex> {
    match entry.compiled.as_ref().map(|compiled| &compiled.matcher) {
        Some(CompiledSanMatcher::Regex(regex)) => Some(regex),
        _ => None,
    }
}

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
    assert!(prepared_regex(&entry).is_some());

    let too_long = AllowedSan {
        san_type: AllowedSanType::Dns,
        match_kind: AllowedSanMatch::RegularExpression,
        value: "a".repeat(ALLOWED_SAN_REGEX_MAX_LEN + 1),
        ignore_case: false,
        oid: None,
        compiled: None,
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
            prepared_regex(&entry).unwrap().is_match(presented),
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
            assert!(entry.compiled.is_none());
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
    entry.compiled = None;
    let error = allowed_san_config_error(&entry).expect("case-folded program must exceed the limit");
    assert!(error.contains("compiled limit"));
    assert_eq!(entry.compile().unwrap_err(), error);
    assert!(entry.compiled.is_none());
}

#[test]
fn compile_prepares_normalized_operands() {
    let mut dns = AllowedSan::exact(AllowedSanType::Dns, "  *.example.com ");
    dns.compile().unwrap();
    let compiled = dns.compiled.as_ref().unwrap();
    assert!(compiled.oid.is_none());
    assert!(matches!(&compiled.matcher, CompiledSanMatcher::DnsExact(v) if v == "*.example.com"));

    let mut ip = AllowedSan::exact(AllowedSanType::Ip, " 2001:0db8::1 ");
    ip.compile().unwrap();
    let expected: IpAddr = "2001:db8::1".parse().unwrap();
    assert!(matches!(&ip.compiled.as_ref().unwrap().matcher, CompiledSanMatcher::IpExact(addr) if *addr == expected));

    let mut other = AllowedSan::other_name(" 1.3.6.1.4.1.311.20.2.3 ", " alice@corp.example.com ");
    other.match_kind = AllowedSanMatch::Suffix;
    other.ignore_case = true;
    other.compile().unwrap();
    let compiled = other.compiled.as_ref().unwrap();
    assert_eq!(compiled.oid.as_deref(), Some("1.3.6.1.4.1.311.20.2.3"));
    assert!(matches!(
        &compiled.matcher,
        CompiledSanMatcher::Text { kind: TextSanMatch::Suffix, value, ignore_case: true }
            if value == "alice@corp.example.com"
    ));

    let mut uri = AllowedSan::exact(AllowedSanType::Uri, "spiffe://cluster.local/ns/a");
    uri.compile().unwrap();
    assert!(matches!(
        &uri.compiled.as_ref().unwrap().matcher,
        CompiledSanMatcher::Text {
            kind: TextSanMatch::Exact,
            ignore_case: false,
            ..
        }
    ));
}

#[test]
fn non_exact_dns_and_ip_prepare_as_text_matchers() {
    for (san_type, kind, value, expected_kind) in [
        (
            AllowedSanType::Ip,
            AllowedSanMatch::Prefix,
            "10.0.",
            TextSanMatch::Prefix,
        ),
        (
            AllowedSanType::Ip,
            AllowedSanMatch::Contains,
            ".0.",
            TextSanMatch::Contains,
        ),
        (
            AllowedSanType::Dns,
            AllowedSanMatch::Suffix,
            ".example.com",
            TextSanMatch::Suffix,
        ),
        (
            AllowedSanType::Dns,
            AllowedSanMatch::Prefix,
            "client",
            TextSanMatch::Prefix,
        ),
    ] {
        let mut entry = AllowedSan::exact(san_type, value);
        entry.match_kind = kind;
        assert!(allowed_san_config_error(&entry).is_none());
        entry.compile().unwrap();
        assert!(
            matches!(
                &entry.compiled.as_ref().unwrap().matcher,
                CompiledSanMatcher::Text { kind, value: v, .. } if *kind == expected_kind && v == value
            ),
            "{san_type:?} {kind:?} must prepare as a text matcher"
        );
    }
}

#[test]
fn compile_rejects_schema_errors_like_admission_and_clears_prepared_state() {
    let mut entry = AllowedSan::exact(AllowedSanType::Dns, "client.example.com");
    entry.compile().unwrap();
    assert!(entry.compiled.is_some());

    let mutations: [fn(&mut AllowedSan); 3] = [
        |e: &mut AllowedSan| e.value = "Client.example.com".to_string(),
        |e: &mut AllowedSan| e.oid = Some("1.2.3".to_string()),
        |e: &mut AllowedSan| e.value = "   ".to_string(),
    ];
    for mutate in mutations {
        let mut invalid = entry.clone();
        mutate(&mut invalid);
        let error = allowed_san_config_error(&invalid).expect("mutation must be invalid");
        assert_eq!(invalid.compile().unwrap_err(), error);
        assert!(
            invalid.compiled.is_none(),
            "a failed compile must not keep a stale matcher"
        );
    }

    let mut bad_ip = AllowedSan::exact(AllowedSanType::Ip, "not-an-ip");
    assert!(bad_ip.compile().is_err());
    assert!(bad_ip.compiled.is_none());
}

#[test]
fn serde_round_trip_drops_prepared_state_and_recompile_restores_it() {
    let mut entry = AllowedSan::exact(AllowedSanType::Uri, "^spiffe://cluster$");
    entry.match_kind = AllowedSanMatch::RegularExpression;
    entry.ignore_case = true;
    entry.compile().unwrap();

    let json = serde_json::to_string(&entry).unwrap();
    assert!(!json.contains("compiled"));
    let mut decoded: AllowedSan = serde_json::from_str(&json).unwrap();
    assert!(decoded.compiled.is_none());
    decoded.compile().unwrap();
    assert!(prepared_regex(&decoded).unwrap().is_match("SPIFFE://CLUSTER"));
}
