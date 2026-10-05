use super::*;

fn dns(value: &str) -> AllowedSan {
    AllowedSan::exact(AllowedSanType::Dns, value)
}

fn compile_regex(san_type: AllowedSanType, pattern: &str) -> AllowedSan {
    let mut entry = AllowedSan::exact(san_type, pattern);
    entry.match_kind = AllowedSanMatch::RegularExpression;
    entry.compile().expect("test regex must compile");
    entry
}

#[test]
fn empty_allowlist_is_unconstrained() {
    assert!(client_sans_match_allowed(
        &[PresentedSan::dns("client.example.com")],
        &[]
    ));
}

#[test]
fn dns_exact_and_case_fold() {
    let allowed = [dns("client.example.com")];
    assert!(client_sans_match_allowed(
        &[PresentedSan::dns("client.example.com")],
        &allowed
    ));
    assert!(client_sans_match_allowed(
        &[PresentedSan::dns("CLIENT.EXAMPLE.COM")],
        &allowed
    ));
    assert!(!client_sans_match_allowed(
        &[PresentedSan::dns("other.example.com")],
        &allowed
    ));
}

#[test]
fn dns_rfc6125_both_directions() {
    let allowlist_wild = [dns("*.internal.example.com")];
    assert!(client_sans_match_allowed(
        &[PresentedSan::dns("a.internal.example.com")],
        &allowlist_wild
    ));
    assert!(!client_sans_match_allowed(
        &[PresentedSan::dns("a.b.internal.example.com")],
        &allowlist_wild
    ));

    let allowlist_exact = [dns("api.example.com")];
    assert!(client_sans_match_allowed(
        &[PresentedSan::dns("*.example.com")],
        &allowlist_exact
    ));
    assert!(!client_sans_match_allowed(
        &[PresentedSan::dns("*.internal.example.com")],
        &allowlist_exact
    ));
}

#[test]
fn cross_type_dns_wildcard_does_not_match_uri() {
    let allowed = [dns("*.internal.example.com")];
    assert!(!client_sans_match_allowed(
        &[PresentedSan::uri("https://a.internal.example.com")],
        &allowed
    ));
}

#[test]
fn dns_value_that_looks_like_uri_does_not_match_uri_allowlist() {
    let allowed = [AllowedSan::exact(AllowedSanType::Uri, "https://a.internal.example.com")];
    assert!(!client_sans_match_allowed(
        &[PresentedSan::dns("https://a.internal.example.com")],
        &allowed
    ));
}

#[test]
fn uri_exact_is_case_sensitive() {
    let allowed = [AllowedSan::exact(
        AllowedSanType::Uri,
        "spiffe://cluster.local/ns/foo/sa/bar",
    )];
    assert!(client_sans_match_allowed(
        &[PresentedSan::uri("spiffe://cluster.local/ns/foo/sa/bar")],
        &allowed
    ));
    assert!(!client_sans_match_allowed(
        &[PresentedSan::uri("SPIFFE://cluster.local/ns/foo/sa/bar")],
        &allowed
    ));
}

#[test]
fn uri_prefix() {
    let mut entry = AllowedSan::exact(AllowedSanType::Uri, "spiffe://cluster.local/");
    entry.match_kind = AllowedSanMatch::Prefix;
    assert!(client_sans_match_allowed(
        &[PresentedSan::uri("spiffe://cluster.local/ns/foo/sa/bar")],
        std::slice::from_ref(&entry)
    ));
    assert!(!client_sans_match_allowed(
        &[PresentedSan::uri("spiffe://other.local/ns/foo/sa/bar")],
        std::slice::from_ref(&entry)
    ));
}

#[test]
fn email_ignore_case_and_suffix() {
    let mut exact = AllowedSan::exact(AllowedSanType::Email, "alice@example.com");
    assert!(!client_sans_match_allowed(
        &[PresentedSan::email("Alice@example.com")],
        std::slice::from_ref(&exact)
    ));
    exact.ignore_case = true;
    assert!(client_sans_match_allowed(
        &[PresentedSan::email("Alice@example.com")],
        std::slice::from_ref(&exact)
    ));

    let mut suffix = AllowedSan::exact(AllowedSanType::Email, "@example.com");
    suffix.match_kind = AllowedSanMatch::Suffix;
    assert!(client_sans_match_allowed(
        &[PresentedSan::email("bob@example.com")],
        std::slice::from_ref(&suffix)
    ));
}

#[test]
fn ip_exact_canonical_and_mapped() {
    let v6 = [AllowedSan::exact(AllowedSanType::Ip, "2001:0db8::1")];
    assert!(client_sans_match_allowed(&[PresentedSan::ip("2001:db8::1")], &v6));
    let mapped = [AllowedSan::exact(AllowedSanType::Ip, "::ffff:127.0.0.1")];
    assert!(!client_sans_match_allowed(&[PresentedSan::ip("127.0.0.1")], &mapped));
    assert!(!client_sans_match_allowed(
        &[PresentedSan::ip("10.0.0.1")],
        &[dns("10.0.0.1")]
    ));
}

#[test]
fn regex_match_requires_precompile() {
    let compiled = compile_regex(AllowedSanType::Dns, r"^client[0-9]+\.example\.com$");
    assert!(client_sans_match_allowed(
        &[PresentedSan::dns("client7.example.com")],
        std::slice::from_ref(&compiled)
    ));
    assert!(!client_sans_match_allowed(
        &[PresentedSan::dns("other.example.com")],
        std::slice::from_ref(&compiled)
    ));

    let mut uncompiled = AllowedSan::exact(AllowedSanType::Dns, r"^client[0-9]+\.example\.com$");
    uncompiled.match_kind = AllowedSanMatch::RegularExpression;
    assert!(
        !client_sans_match_allowed(
            &[PresentedSan::dns("client7.example.com")],
            std::slice::from_ref(&uncompiled)
        ),
        "uncompiled regex must fail closed"
    );
}

#[test]
fn other_name_matches_oid_and_value() {
    let allowed = [AllowedSan::other_name(
        "1.3.6.1.4.1.311.20.2.3",
        "alice@corp.example.com",
    )];
    assert!(client_sans_match_allowed(
        &[PresentedSan::other_name(
            "1.3.6.1.4.1.311.20.2.3",
            "alice@corp.example.com"
        )],
        &allowed
    ));
    assert!(!client_sans_match_allowed(
        &[PresentedSan::other_name("1.2.3", "alice@corp.example.com")],
        &allowed
    ));
    assert!(!client_sans_match_allowed(
        &[PresentedSan::other_name(
            "1.3.6.1.4.1.311.20.2.3",
            "bob@corp.example.com"
        )],
        &allowed
    ));
}

#[test]
fn invalid_entry_fails_closed() {
    let bad = [AllowedSan::exact(AllowedSanType::Ip, "not-an-ip")];
    assert!(!client_sans_match_allowed(&[PresentedSan::ip("127.0.0.1")], &bad));
}

#[test]
fn typed_display_prefix_covers_other_name() {
    assert!(!has_typed_san_display_prefix("client.example.com"));
    assert!(has_typed_san_display_prefix("IP:127.0.0.1"));
    assert!(has_typed_san_display_prefix("uri:spiffe://cluster.local/ns/foo"));
    assert!(has_typed_san_display_prefix("email:alice@example.com"));
    assert!(has_typed_san_display_prefix(
        "otherName:1.3.6.1.4.1.311.20.2.3=alice@corp.example.com"
    ));
}

#[test]
fn regex_ignore_case_matches_only_the_selected_identity_type() {
    for (ignore_case, pattern, expected) in [
        (false, r"^spiffe://cluster$", false),
        (true, r"^spiffe://cluster$", true),
        (false, r"(?i)^spiffe://cluster$", true),
        (true, r"(?-i)^spiffe://cluster$", false),
    ] {
        let mut entry = AllowedSan::exact(AllowedSanType::Uri, pattern);
        entry.match_kind = AllowedSanMatch::RegularExpression;
        entry.ignore_case = ignore_case;
        assert!(!client_sans_match_allowed(
            &[PresentedSan::uri("SPIFFE://CLUSTER")],
            std::slice::from_ref(&entry)
        ));
        entry.compile().unwrap();
        assert_eq!(
            client_sans_match_allowed(&[PresentedSan::uri("SPIFFE://CLUSTER")], std::slice::from_ref(&entry)),
            expected
        );
        assert!(!client_sans_match_allowed(
            &[PresentedSan::dns("SPIFFE://CLUSTER")],
            std::slice::from_ref(&entry)
        ));

        let mut other_name = AllowedSan::other_name("1.2.3", pattern);
        other_name.match_kind = AllowedSanMatch::RegularExpression;
        other_name.ignore_case = ignore_case;
        other_name.compile().unwrap();
        assert_eq!(
            client_sans_match_allowed(
                &[PresentedSan::other_name("1.2.3", "SPIFFE://CLUSTER")],
                std::slice::from_ref(&other_name)
            ),
            expected
        );
        assert!(!client_sans_match_allowed(
            &[PresentedSan::other_name("1.2.4", "SPIFFE://CLUSTER")],
            std::slice::from_ref(&other_name)
        ));
    }
}
