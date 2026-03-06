use crate::types::schema::is_valid_domain;
use std::collections::HashMap;

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
        let key = if let Some(rest) = k.strip_prefix("*.") {
            if is_valid_domain(rest) {
                format!(".{}", rest) // "*.aaa.com" -> ".aaa.com" to distinguish from "aaa.com"
            } else {
                return false;
            }
        } else if is_valid_domain(k) {
            k.to_string()
        } else {
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
        if !is_valid_domain(k) {
            return None;
        }

        if self.map.contains_key(k) {
            return self.map.get_mut(k);
        }

        // Optimized wildcard lookup (same strategy as get)
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

    pub fn remove(&mut self, k: &str) -> Option<T> {
        self.map.remove(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
