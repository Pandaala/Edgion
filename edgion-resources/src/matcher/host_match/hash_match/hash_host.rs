use crate::schema::is_valid_domain;
use std::collections::HashMap;

/// Normalize a hostname pattern to its storage key, rejecting invalid domains.
/// Wildcards use `.example.com` keys; exact names use `example.com` keys.
pub fn canonical_host_key(hostname: &str) -> Option<String> {
    let lower = hostname.to_lowercase();
    if let Some(rest) = lower.strip_prefix("*.") {
        is_valid_domain(rest).then(|| format!(".{rest}"))
    } else {
        is_valid_domain(&lower).then_some(lower)
    }
}

/// Look up an exact hostname, then a single-label wildcard per RFC 6125 §6.4.3.
/// The callback reads keys produced by [`canonical_host_key`] from any storage.
/// Already lowercase ASCII hostnames require no allocation.
pub fn lookup_rfc6125<'a, T: 'a>(hostname: &str, mut lookup: impl FnMut(&str) -> Option<&'a T>) -> Option<&'a T> {
    let lower;
    let hostname = if hostname.bytes().any(|b| b.is_ascii_uppercase()) {
        lower = hostname.to_ascii_lowercase();
        lower.as_str()
    } else {
        hostname
    };
    if !is_valid_domain(hostname) {
        return None;
    }
    if let Some(value) = lookup(hostname) {
        return Some(value);
    }
    // Strip exactly one label, preserving the wildcard key's leading dot.
    let dot = hostname.find('.')?;
    lookup(&hostname[dot..])
}

/// Hash-based hostname matcher with exact and wildcard support.
///
/// Wildcard patterns (`*.example.com`) are stored as `.example.com` keys.
/// Lookup checks exact match first, then wildcard suffixes (most specific first).
///
/// ## Wildcard lookup optimization
///
/// Instead of scanning every dot-level suffix of the query hostname, we track
/// the set of distinct wildcard key lengths inserted so far. During lookup we
/// only extract the suffixes whose lengths actually appear in the map, turning
/// O(dot_depth) HashMap probes into O(W) where W is the number of unique
/// wildcard suffix lengths (typically 1-2).
#[derive(Clone)]
pub struct HashHost<T> {
    map: HashMap<String, T>,
    /// Distinct lengths of wildcard keys (e.g., `.example.com` -> 12), sorted
    /// **descending** so that the most-specific (longest) suffix is checked first.
    wildcard_key_lens: Vec<usize>,
}

impl<T> Default for HashHost<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> HashHost<T> {
    pub fn new() -> HashHost<T> {
        HashHost {
            map: HashMap::new(),
            wildcard_key_lens: Vec::new(),
        }
    }

    pub fn insert(&mut self, k: &str, v: T) -> bool {
        let Some(key) = canonical_host_key(k) else {
            return false;
        };

        // Track wildcard key length for optimized lookup
        if key.starts_with('.') {
            let len = key.len();
            if let Err(pos) = self.wildcard_key_lens.binary_search_by(|a| len.cmp(a)) {
                self.wildcard_key_lens.insert(pos, len);
            }
        }

        self.map.insert(key, v);
        true
    }

    pub fn get(&self, k: &str) -> Option<&T> {
        // Avoid heap allocation when input is already ASCII-lowercase (the common
        // case: HTTP/2 headers and TLS SNI names are always lowercase per spec).
        // DNS hostnames are ASCII-only, so to_ascii_lowercase is sufficient.
        if k.bytes().any(|b| b.is_ascii_uppercase()) {
            self.get_lower(&k.to_ascii_lowercase())
        } else {
            self.get_lower(k)
        }
    }

    /// Like `get`, but the wildcard bucket only matches a **single** left-most
    /// label per RFC 6125 §6.4.3. Use this for TLS certificate selection.
    ///
    /// `get_rfc6125("foo.example.com")` matches a registered `*.example.com`;
    /// `get_rfc6125("a.b.example.com")` does NOT — that would be a multi-label
    /// match, which RFC 6125-compliant client TLS stacks reject during the
    /// handshake. Route matching (which is governed by Gateway API §4.3) still
    /// uses [`HashHost::get`], so the two semantics coexist intentionally.
    pub fn get_rfc6125(&self, k: &str) -> Option<&T> {
        lookup_rfc6125(k, |key| self.map.get(key))
    }

    fn get_lower(&self, k: &str) -> Option<&T> {
        if !is_valid_domain(k) {
            return None;
        }
        if let Some(value) = self.map.get(k) {
            return Some(value);
        }
        // Optimized wildcard lookup: only probe suffix lengths that actually
        // exist among the registered wildcard keys (longest first).
        let k_len = k.len();
        for &wk_len in &self.wildcard_key_lens {
            if k_len > wk_len {
                let suffix = &k[k_len - wk_len..];
                if let Some(value) = self.map.get(suffix) {
                    return Some(value);
                }
            }
        }
        None
    }

    pub fn get_mut(&mut self, k: &str) -> Option<&mut T> {
        if k.bytes().any(|b| b.is_ascii_uppercase()) {
            self.get_mut_lower(&k.to_ascii_lowercase())
        } else {
            self.get_mut_lower(k)
        }
    }

    fn get_mut_lower(&mut self, k: &str) -> Option<&mut T> {
        if !is_valid_domain(k) {
            return None;
        }
        if self.map.contains_key(k) {
            return self.map.get_mut(k);
        }
        // Optimized wildcard lookup (same strategy as get_lower)
        let k_len = k.len();
        let mut matched_start = None;
        for &wk_len in &self.wildcard_key_lens {
            if k_len > wk_len {
                let start = k_len - wk_len;
                if self.map.contains_key(&k[start..]) {
                    matched_start = Some(start);
                    break;
                }
            }
        }
        matched_start.and_then(move |pos| self.map.get_mut(&k[pos..]))
    }

    /// Exact-pattern mutable lookup: returns `Some` only for the bucket stored
    /// under the **same pattern** `k` was inserted as. Does **not** fall back to
    /// wildcard suffix matching (so an exact hostname never merges into a wildcard
    /// bucket), but it DOES resolve a wildcard pattern to its own bucket: just like
    /// [`HashHost::insert`], `"*.aaa.com"` normalizes to the `".aaa.com"` key. This
    /// symmetry lets a store builder accumulate multiple entries for the same
    /// wildcard pattern instead of silently overwriting (`is_valid_domain` rejects
    /// the raw `"*"` label, so without this branch a wildcard key always missed).
    /// Use this when building a store to avoid accidentally merging a new hostname
    /// entry into an existing wildcard bucket.
    pub fn get_mut_exact(&mut self, k: &str) -> Option<&mut T> {
        let key = canonical_host_key(k)?;
        self.map.get_mut(&key)
    }

    pub fn remove(&mut self, k: &str) -> Option<T> {
        self.map.remove(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_host_key_rejects_invalid_patterns() {
        assert_eq!(canonical_host_key("API.Example.COM"), Some("api.example.com".into()));
        assert_eq!(canonical_host_key("*.Example.COM"), Some(".example.com".into()));
        for invalid in [
            "",
            "*",
            "*.",
            ".example.com",
            "*.*.example.com",
            "foo..example.com",
            "é.example.com",
        ] {
            assert_eq!(canonical_host_key(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn test_lookup_rfc6125_bounds_storage_probes() {
        let exact = 1;
        let wildcard = 2;
        let mut keys = Vec::new();
        assert_eq!(
            lookup_rfc6125("API.Example.COM", |key| {
                keys.push(key.to_owned());
                (key == "api.example.com").then_some(&exact)
            }),
            Some(&exact)
        );
        assert_eq!(keys, ["api.example.com"]);

        keys.clear();
        assert_eq!(
            lookup_rfc6125("api.example.com", |key| {
                keys.push(key.to_owned());
                (key == ".example.com").then_some(&wildcard)
            }),
            Some(&wildcard)
        );
        assert_eq!(keys, ["api.example.com", ".example.com"]);

        keys.clear();
        assert_eq!(
            lookup_rfc6125("deep.api.example.com", |key| {
                keys.push(key.to_owned());
                (key == ".example.com").then_some(&wildcard)
            }),
            None
        );
        assert_eq!(keys, ["deep.api.example.com", ".api.example.com"]);

        for invalid in ["", "*.example.com", "foo..example.com", "é.example.com"] {
            assert_eq!(
                lookup_rfc6125(invalid, |_| panic!("invalid hostname reached storage")),
                None::<&u8>
            );
        }
    }

    #[test]
    fn test_exact_match() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("example.com", "exact".to_string());

        assert_eq!(host.get("example.com"), Some(&"exact".to_string()));
        assert_eq!(host.get("other.com"), None);
    }

    #[test]
    fn test_wildcard_single_level() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("*.bar.com", "wildcard".to_string());

        assert_eq!(host.get("foo.bar.com"), Some(&"wildcard".to_string()));
        assert_eq!(host.get("bar.com"), None);
    }

    #[test]
    fn test_wildcard_multi_level() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("*.bar.com", "wildcard".to_string());

        assert_eq!(host.get("a.b.bar.com"), Some(&"wildcard".to_string()));
        assert_eq!(host.get("x.y.z.bar.com"), Some(&"wildcard".to_string()));
    }

    #[test]
    fn test_exact_takes_priority_over_wildcard() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("foo.bar.com", "exact".to_string());
        host.insert("*.bar.com", "wildcard".to_string());

        assert_eq!(host.get("foo.bar.com"), Some(&"exact".to_string()));
        assert_eq!(host.get("other.bar.com"), Some(&"wildcard".to_string()));
    }

    #[test]
    fn test_wildcard_key_lens_tracking() {
        let mut host: HashHost<String> = HashHost::new();
        assert!(host.wildcard_key_lens.is_empty());

        host.insert("*.example.com", "w1".to_string());
        // ".example.com" = 12 chars
        assert_eq!(host.wildcard_key_lens, vec![12]);

        host.insert("*.api.example.com", "w2".to_string());
        // ".api.example.com" = 16 chars, sorted descending -> [16, 12]
        assert_eq!(host.wildcard_key_lens, vec![16, 12]);

        // Exact-only insert should not add to wildcard_key_lens
        host.insert("test.com", "exact".to_string());
        assert_eq!(host.wildcard_key_lens, vec![16, 12]);
    }

    #[test]
    fn test_most_specific_wildcard_wins() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("*.example.com", "broad".to_string());
        host.insert("*.api.example.com", "specific".to_string());

        assert_eq!(host.get("v1.api.example.com"), Some(&"specific".to_string()));
        assert_eq!(host.get("web.example.com"), Some(&"broad".to_string()));
        // Multi-level: deeper subdomain still picks most specific wildcard
        assert_eq!(host.get("x.y.api.example.com"), Some(&"specific".to_string()));
    }

    #[test]
    fn test_deep_hostname_no_unnecessary_probes() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("*.example.com", "wc".to_string());
        // Only 1 wildcard key length tracked
        assert_eq!(host.wildcard_key_lens.len(), 1);

        // Even with a very deep hostname, only 1 HashMap probe for wildcard
        assert_eq!(host.get("a.b.c.d.e.f.g.example.com"), Some(&"wc".to_string()));
        // Non-matching deep hostname: still only 1 probe
        assert_eq!(host.get("a.b.c.d.e.f.g.other.com"), None);
    }

    #[test]
    fn test_no_wildcards_skips_scan() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("example.com", "exact".to_string());
        host.insert("other.com", "other".to_string());

        assert!(host.wildcard_key_lens.is_empty());
        assert_eq!(host.get("example.com"), Some(&"exact".to_string()));
        assert_eq!(host.get("foo.example.com"), None);
    }

    #[test]
    fn test_get_mut_with_optimization() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("*.example.com", "original".to_string());

        if let Some(v) = host.get_mut("api.example.com") {
            *v = "modified".to_string();
        }

        assert_eq!(host.get("api.example.com"), Some(&"modified".to_string()));
        assert_eq!(host.get("web.example.com"), Some(&"modified".to_string()));
    }

    #[test]
    fn test_get_mut_exact_does_not_match_wildcard() {
        let mut host: HashHost<Vec<&str>> = HashHost::new();
        host.insert("*.wildcard.org", vec!["wildcard"]);

        // get_mut would return the wildcard bucket; get_mut_exact must not.
        assert!(
            host.get_mut_exact("fourth-example.wildcard.org").is_none(),
            "get_mut_exact must not fall back to wildcard"
        );
        assert!(
            host.get_mut("fourth-example.wildcard.org").is_some(),
            "get_mut should still fall back to wildcard"
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
        let mut host: HashHost<Vec<&str>> = HashHost::new();
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
        let mut host: HashHost<String> = HashHost::new();
        // ".example.com" and ".testing.com" have different content but same length (12)
        host.insert("*.example.com", "ex".to_string());
        host.insert("*.testing.com", "te".to_string());

        // Only one entry in wildcard_key_lens (deduplicated)
        assert_eq!(host.wildcard_key_lens, vec![12]);

        assert_eq!(host.get("api.example.com"), Some(&"ex".to_string()));
        assert_eq!(host.get("api.testing.com"), Some(&"te".to_string()));
        assert_eq!(host.get("api.unknown.com"), None);
    }

    #[test]
    fn test_get_rfc6125_single_label() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("*.example.com", "wildcard".to_string());

        // Single-label match — RFC 6125 §6.4.3 OK
        assert_eq!(host.get_rfc6125("foo.example.com"), Some(&"wildcard".to_string()));

        // Multi-label match — RFC 6125 §6.4.3 forbids
        assert_eq!(host.get_rfc6125("a.b.example.com"), None);

        // Root domain — wildcard does not cover bare suffix
        assert_eq!(host.get_rfc6125("example.com"), None);

        // Case-insensitive — RFC 6125 §6.4.1
        assert_eq!(host.get_rfc6125("Foo.Example.COM"), Some(&"wildcard".to_string()));
    }

    #[test]
    fn test_get_rfc6125_most_specific() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("*.example.com", "broad".to_string());
        host.insert("*.api.example.com", "specific".to_string());

        // Most-specific single-label match wins
        assert_eq!(host.get_rfc6125("v1.api.example.com"), Some(&"specific".to_string()));

        // Single label against the broad wildcard
        assert_eq!(host.get_rfc6125("web.example.com"), Some(&"broad".to_string()));

        // Four-label host — neither wildcard's single-label rule covers it.
        // `*.api.example.com` would need exactly one label before `api.example.com`
        // (`x.y` is two labels). `*.example.com` would need exactly one label before
        // `example.com` (`x.y.api` is three labels).
        assert_eq!(host.get_rfc6125("x.y.api.example.com"), None);
    }

    #[test]
    fn test_get_rfc6125_exact_priority() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("foo.example.com", "exact".to_string());
        host.insert("*.example.com", "wildcard".to_string());

        // Exact match wins over wildcard
        assert_eq!(host.get_rfc6125("foo.example.com"), Some(&"exact".to_string()));
        // Other subdomains still hit the wildcard
        assert_eq!(host.get_rfc6125("bar.example.com"), Some(&"wildcard".to_string()));
    }

    #[test]
    fn test_get_rfc6125_single_label_sni_returns_none() {
        let mut host: HashHost<String> = HashHost::new();
        host.insert("*.example.com", "wildcard".to_string());

        // SNI with no dot at all — RFC 6125 wildcards require at least one
        // dot in the query; bare hostnames cannot match a wildcard pattern.
        assert_eq!(host.get_rfc6125("localhost"), None);
    }

    #[test]
    fn test_get_vs_get_rfc6125_divergence() {
        // The two accessors deliberately diverge for multi-label SNI: `get` is the
        // route-matching path (Gateway API §4.3, multi-label), `get_rfc6125` is the
        // TLS cert-selection path (RFC 6125 §6.4.3, single-label). This test pins
        // the invariant the doc-comment advertises so neither side can drift.
        let mut host: HashHost<String> = HashHost::new();
        host.insert("*.example.com", "wildcard".to_string());

        // `get` (multi-label) — finds the wildcard for any subdomain depth
        assert_eq!(host.get("a.b.example.com"), Some(&"wildcard".to_string()));
        // `get_rfc6125` (single-label) — same input returns None
        assert_eq!(host.get_rfc6125("a.b.example.com"), None);

        // Single-label query: both agree
        assert_eq!(host.get("foo.example.com"), Some(&"wildcard".to_string()));
        assert_eq!(host.get_rfc6125("foo.example.com"), Some(&"wildcard".to_string()));
    }
}
