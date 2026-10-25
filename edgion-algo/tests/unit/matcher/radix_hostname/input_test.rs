use super::*;

#[test]
fn normalized_queries_borrow_and_sni_case_folding_is_separate() {
    assert!(matches!(
        normalize_route_sni("a.b.example.com").unwrap().0,
        Cow::Borrowed(_)
    ));
    assert_eq!(
        normalize_route_sni("A.B.Example.COM").unwrap().query().as_str(),
        "a.b.example.com"
    );
    assert!(RadixHostnameQuery::new("UPPER.test").is_err());
    assert!(RadixHostnameQuery::new("grpc").is_ok());
    for len in [252, 253] {
        let lower = "a".repeat(len);
        let upper = "A".repeat(len);
        assert_eq!(normalize_route_sni(&lower).unwrap().query().as_str(), lower);
        assert_eq!(normalize_route_sni(&upper).unwrap().query().as_str(), lower);
    }
}

#[test]
fn invalid_sni_is_not_repaired_or_treated_as_missing() {
    for value in [
        "",
        "*",
        "*.test",
        ".test",
        "a..test",
        "a.test.",
        "a.test:443",
        " a.test",
        "a.test ",
        "é.test",
        "127.0.0.1",
        "[::1]",
    ] {
        assert!(normalize_route_sni(value).is_err(), "{value}");
    }
    for value in [
        String::new(),
        "a".repeat(254),
        "A".repeat(254),
        format!("*.{}", "A".repeat(252)),
        format!("{}é", "A".repeat(252)),
        "A".repeat(16_384),
    ] {
        assert!(matches!(
            normalize_route_sni(&value),
            Err(RadixHostnamePatternError::InvalidLength)
        ));
    }
    assert!(matches!(
        normalize_route_sni(&format!("{}é", "A".repeat(251))),
        Err(RadixHostnamePatternError::InvalidPattern)
    ));
}
