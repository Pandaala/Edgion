//! RFC 6125 single-label wildcard matching for certificate selection.
//!
//! This policy intentionally differs from the sibling Route hostname radix,
//! whose Gateway API wildcards may cover multiple labels.

use std::collections::HashMap;

/// Match a configured certificate pattern against one hostname.
pub fn matches(pattern: &str, hostname: &str) -> bool {
    if hostname.eq_ignore_ascii_case(pattern) {
        return true;
    }
    let Some(suffix) = pattern.strip_prefix("*.") else {
        return false;
    };
    if suffix.is_empty() || hostname.len() < suffix.len() + 2 {
        return false;
    }
    let split_pos = hostname.len() - suffix.len();
    if hostname.as_bytes()[split_pos - 1] != b'.' || !hostname[split_pos..].eq_ignore_ascii_case(suffix) {
        return false;
    }
    let leftmost_label = &hostname[..split_pos - 1];
    !leftmost_label.is_empty() && !leftmost_label.contains('.')
}

fn is_valid_hostname(hostname: &str) -> bool {
    let labels = hostname.split('.');
    let mut label_count = 0usize;
    for label in labels {
        if label.is_empty()
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return false;
        }
        label_count += 1;
    }
    label_count >= 2
}

/// Normalize a hostname pattern to its storage key, rejecting invalid domains.
/// Wildcards use `.example.com` keys; exact names use `example.com` keys.
pub fn canonical_key(hostname: &str) -> Option<String> {
    let lower = hostname.to_lowercase();
    if let Some(rest) = lower.strip_prefix("*.") {
        is_valid_hostname(rest).then(|| format!(".{rest}"))
    } else {
        is_valid_hostname(&lower).then_some(lower)
    }
}

/// Look up an exact hostname, then a single-label wildcard per RFC 6125 §6.4.3.
/// The callback reads keys produced by [`canonical_key`] from any storage.
/// Already lowercase ASCII hostnames require no allocation.
pub fn lookup<'a, T: 'a>(hostname: &str, mut lookup_key: impl FnMut(&str) -> Option<&'a T>) -> Option<&'a T> {
    let lower;
    let hostname = if hostname.bytes().any(|b| b.is_ascii_uppercase()) {
        lower = hostname.to_ascii_lowercase();
        lower.as_str()
    } else {
        hostname
    };
    if !is_valid_hostname(hostname) {
        return None;
    }
    if let Some(value) = lookup_key(hostname) {
        return Some(value);
    }
    // Strip exactly one label, preserving the wildcard key's leading dot.
    let dot = hostname.find('.')?;
    lookup_key(&hostname[dot..])
}

/// Hash-based RFC 6125 hostname matcher with exact and single-label wildcard support.
#[derive(Clone)]
pub struct Rfc6125HostnameMatcher<T> {
    map: HashMap<String, T>,
}

impl<T> Default for Rfc6125HostnameMatcher<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Rfc6125HostnameMatcher<T> {
    pub fn new() -> Rfc6125HostnameMatcher<T> {
        Rfc6125HostnameMatcher { map: HashMap::new() }
    }

    pub fn insert(&mut self, k: &str, v: T) -> bool {
        let Some(key) = canonical_key(k) else {
            return false;
        };

        self.map.insert(key, v);
        true
    }

    /// Return an exact match first, then consume exactly one left-most label for
    /// wildcard fallback per RFC 6125 section 6.4.3.
    pub fn get(&self, k: &str) -> Option<&T> {
        lookup(k, |key| self.map.get(key))
    }

    /// Exact-pattern mutable lookup: returns `Some` only for the bucket stored
    /// under the **same pattern** `k` was inserted as. Does **not** fall back to
    /// wildcard suffix matching (so an exact hostname never merges into a wildcard
    /// bucket), but it does resolve a wildcard pattern to its own bucket: just like
    /// [`Rfc6125HostnameMatcher::insert`], `"*.aaa.com"` normalizes to the `".aaa.com"` key. This
    /// symmetry lets a store builder accumulate multiple entries for the same
    /// wildcard pattern instead of silently overwriting (`is_valid_hostname` rejects
    /// the raw `"*"` label, so without this branch a wildcard key always missed).
    /// Use this when building a store to avoid accidentally merging a new hostname
    /// entry into an existing wildcard bucket.
    pub fn get_mut_exact(&mut self, k: &str) -> Option<&mut T> {
        let key = canonical_key(k)?;
        self.map.get_mut(&key)
    }
}

#[cfg(test)]
mod tests {
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
}
