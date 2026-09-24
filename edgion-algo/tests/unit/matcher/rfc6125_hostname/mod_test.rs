use super::*;

#[test]
fn test_matches_uses_exactly_one_wildcard_label() {
    assert!(matches("api.example.com", "API.EXAMPLE.COM"));
    assert!(matches("*.example.com", "api.example.com"));
    assert!(!matches("*.example.com", "a.api.example.com"));
    assert!(!matches("*.example.com", "example.com"));
}

#[test]
fn test_canonical_key_rejects_invalid_patterns() {
    assert_eq!(canonical_key("API.Example.COM"), Some("api.example.com".into()));
    assert_eq!(canonical_key("*.Example.COM"), Some(".example.com".into()));
    for invalid in [
        "",
        "*",
        "*.",
        ".example.com",
        "*.*.example.com",
        "foo..example.com",
        "é.example.com",
    ] {
        assert_eq!(canonical_key(invalid), None, "{invalid}");
    }
}

#[test]
fn test_lookup_bounds_storage_probes() {
    let exact = 1;
    let wildcard = 2;
    let mut keys = Vec::new();
    assert_eq!(
        lookup("API.Example.COM", |key| {
            keys.push(key.to_owned());
            (key == "api.example.com").then_some(&exact)
        }),
        Some(&exact)
    );
    assert_eq!(keys, ["api.example.com"]);

    keys.clear();
    assert_eq!(
        lookup("api.example.com", |key| {
            keys.push(key.to_owned());
            (key == ".example.com").then_some(&wildcard)
        }),
        Some(&wildcard)
    );
    assert_eq!(keys, ["api.example.com", ".example.com"]);

    keys.clear();
    assert_eq!(
        lookup("deep.api.example.com", |key| {
            keys.push(key.to_owned());
            (key == ".example.com").then_some(&wildcard)
        }),
        None
    );
    assert_eq!(keys, ["deep.api.example.com", ".api.example.com"]);

    for invalid in ["", "*.example.com", "foo..example.com", "é.example.com"] {
        assert_eq!(
            lookup(invalid, |_| panic!("invalid hostname reached storage")),
            None::<&u8>
        );
    }
}

#[test]
fn test_exact_match() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("example.com", "exact".to_string());

    assert_eq!(host.get("example.com"), Some(&"exact".to_string()));
    assert_eq!(host.get("other.com"), None);
}

#[test]
fn test_wildcard_single_level() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("*.bar.com", "wildcard".to_string());

    assert_eq!(host.get("foo.bar.com"), Some(&"wildcard".to_string()));
    assert_eq!(host.get("bar.com"), None);
}

#[test]
fn test_wildcard_rejects_multiple_labels() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("*.bar.com", "wildcard".to_string());

    assert_eq!(host.get("a.b.bar.com"), None);
    assert_eq!(host.get("x.y.z.bar.com"), None);
}

#[test]
fn test_exact_takes_priority_over_wildcard() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("foo.bar.com", "exact".to_string());
    host.insert("*.bar.com", "wildcard".to_string());

    assert_eq!(host.get("foo.bar.com"), Some(&"exact".to_string()));
    assert_eq!(host.get("other.bar.com"), Some(&"wildcard".to_string()));
}

#[test]
fn test_most_specific_wildcard_wins() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("*.example.com", "broad".to_string());
    host.insert("*.api.example.com", "specific".to_string());

    assert_eq!(host.get("v1.api.example.com"), Some(&"specific".to_string()));
    assert_eq!(host.get("web.example.com"), Some(&"broad".to_string()));
    assert_eq!(host.get("x.y.api.example.com"), None);
}

#[test]
fn test_no_wildcards_skips_scan() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("example.com", "exact".to_string());
    host.insert("other.com", "other".to_string());

    assert_eq!(host.get("example.com"), Some(&"exact".to_string()));
    assert_eq!(host.get("foo.example.com"), None);
}

#[test]
fn test_get_mut_exact_does_not_match_wildcard() {
    let mut host: Rfc6125HostnameMatcher<Vec<&str>> = Rfc6125HostnameMatcher::new();
    host.insert("*.wildcard.org", vec!["wildcard"]);

    assert!(
        host.get_mut_exact("fourth-example.wildcard.org").is_none(),
        "get_mut_exact must not fall back to wildcard"
    );

    // After inserting the exact key, get_mut_exact should find it.
    host.insert("fourth-example.wildcard.org", vec!["exact"]);
    assert_eq!(
        host.get_mut_exact("fourth-example.wildcard.org"),
        Some(&mut vec!["exact"]),
    );
    // get (read) should also prefer exact over wildcard.
    assert_eq!(host.get("fourth-example.wildcard.org"), Some(&vec!["exact"]));
}

#[test]
fn test_get_mut_exact_resolves_wildcard_pattern() {
    let mut host: Rfc6125HostnameMatcher<Vec<&str>> = Rfc6125HostnameMatcher::new();
    host.insert("*.example.com", vec!["w1"]);

    // A wildcard pattern resolves to its own bucket (symmetric with insert),
    // so a store builder can accumulate instead of overwriting.
    let bucket = host
        .get_mut_exact("*.example.com")
        .expect("get_mut_exact must resolve the wildcard pattern to its own bucket");
    bucket.push("w2");
    assert_eq!(host.get_mut_exact("*.example.com"), Some(&mut vec!["w1", "w2"]));

    // It must still NOT fall back to wildcard for a concrete subdomain.
    assert!(host.get_mut_exact("foo.example.com").is_none());

    // Case-insensitive, mirroring insert.
    assert!(host.get_mut_exact("*.EXAMPLE.com").is_some());

    // An invalid wildcard rest returns None, not a panic.
    assert!(host.get_mut_exact("*.").is_none());
}

#[test]
fn test_duplicate_wildcard_lengths() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    // ".example.com" and ".testing.com" have different content but same length (12)
    host.insert("*.example.com", "ex".to_string());
    host.insert("*.testing.com", "te".to_string());

    assert_eq!(host.get("api.example.com"), Some(&"ex".to_string()));
    assert_eq!(host.get("api.testing.com"), Some(&"te".to_string()));
    assert_eq!(host.get("api.unknown.com"), None);
}

#[test]
fn test_get_single_label() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("*.example.com", "wildcard".to_string());

    // Single-label match — RFC 6125 §6.4.3 OK
    assert_eq!(host.get("foo.example.com"), Some(&"wildcard".to_string()));

    // Multi-label match — RFC 6125 §6.4.3 forbids
    assert_eq!(host.get("a.b.example.com"), None);

    // Root domain — wildcard does not cover bare suffix
    assert_eq!(host.get("example.com"), None);

    // Case-insensitive — RFC 6125 §6.4.1
    assert_eq!(host.get("Foo.Example.COM"), Some(&"wildcard".to_string()));
}

#[test]
fn test_get_most_specific() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("*.example.com", "broad".to_string());
    host.insert("*.api.example.com", "specific".to_string());

    // Most-specific single-label match wins
    assert_eq!(host.get("v1.api.example.com"), Some(&"specific".to_string()));

    // Single label against the broad wildcard
    assert_eq!(host.get("web.example.com"), Some(&"broad".to_string()));

    // Four-label host — neither wildcard's single-label rule covers it.
    // `*.api.example.com` would need exactly one label before `api.example.com`
    // (`x.y` is two labels). `*.example.com` would need exactly one label before
    // `example.com` (`x.y.api` is three labels).
    assert_eq!(host.get("x.y.api.example.com"), None);
}

#[test]
fn test_get_exact_priority() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("foo.example.com", "exact".to_string());
    host.insert("*.example.com", "wildcard".to_string());

    // Exact match wins over wildcard
    assert_eq!(host.get("foo.example.com"), Some(&"exact".to_string()));
    // Other subdomains still hit the wildcard
    assert_eq!(host.get("bar.example.com"), Some(&"wildcard".to_string()));
}

#[test]
fn test_get_single_label_sni_returns_none() {
    let mut host: Rfc6125HostnameMatcher<String> = Rfc6125HostnameMatcher::new();
    host.insert("*.example.com", "wildcard".to_string());

    // SNI with no dot at all — RFC 6125 wildcards require at least one
    // dot in the query; bare hostnames cannot match a wildcard pattern.
    assert_eq!(host.get("localhost"), None);
}
