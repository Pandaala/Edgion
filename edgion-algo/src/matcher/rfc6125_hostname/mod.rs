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
#[path = "../../../tests/unit/matcher/rfc6125_hostname/mod_test.rs"]
mod tests;
