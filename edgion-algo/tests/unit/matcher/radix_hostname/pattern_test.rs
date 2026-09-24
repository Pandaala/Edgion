use super::*;

#[test]
fn gateway_api_hostname_grammar_and_lengths() {
    for value in ["a", "a-b", "example.com", "*.a", "*.example.com", "*.127.0.0.1"] {
        assert!(RadixHostnamePatternRef::parse(value).is_ok(), "{value}");
    }
    for value in [
        "",
        "*",
        "*.",
        "foo.*.com",
        "example.com.",
        ".example.com",
        "a..b",
        "-a",
        "a-",
        "A.example.com",
        "a_b",
        "a:80",
        "127.0.0.1",
        "::1",
        "[::1]",
        "é.example",
        " a",
    ] {
        assert!(RadixHostnamePatternRef::parse(value).is_err(), "{value}");
    }
    assert_eq!(
        RadixHostnamePatternRef::parse("127.0.0.1"),
        Err(RadixHostnamePatternError::IpLiteral)
    );
    assert!(RadixHostnamePatternRef::parse(&"a".repeat(253)).is_ok());
    assert!(RadixHostnamePatternRef::parse(&"a".repeat(254)).is_err());
    assert!(RadixHostnamePatternRef::parse(&format!("*.{}", "a".repeat(251))).is_ok());
    assert!(RadixHostnamePatternRef::parse(&format!("*.{}", "a".repeat(252))).is_err());
}

#[test]
fn wildcard_matches_preserve_label_boundaries_and_require_a_prefix() {
    let pattern = RadixHostnamePatternRef::parse("*.example.com").unwrap();
    assert!(pattern.is_wildcard());
    assert_eq!(pattern.suffix(), Some(".example.com"));
    for name in ["a.example.com", "a.b.example.com"] {
        assert!(pattern.matches(name));
    }
    for name in ["example.com", "badexample.com", ".example.com", "example.org"] {
        assert!(!pattern.matches(name));
    }
}

#[test]
fn intersections_are_commutative_and_return_the_narrower_input() {
    for (left, right, expected) in [
        ("a", "a", Some("a")),
        ("a", "b", None),
        ("*.example.com", "a.example.com", Some("a.example.com")),
        ("*.example.com", "a.b.example.com", Some("a.b.example.com")),
        ("*.example.com", "example.com", None),
        ("*.example.com", "*.b.example.com", Some("*.b.example.com")),
        ("*.example.com", "*.badexample.com", None),
        ("*.a", "a.a", Some("a.a")),
    ] {
        let left = RadixHostnamePatternRef::parse(left).unwrap();
        let right = RadixHostnamePatternRef::parse(right).unwrap();
        assert_eq!(left.intersection(right).map(RadixHostnamePatternRef::as_str), expected);
        assert_eq!(right.intersection(left).map(RadixHostnamePatternRef::as_str), expected);
        assert_eq!(left.intersection(left), Some(left));
    }
}
