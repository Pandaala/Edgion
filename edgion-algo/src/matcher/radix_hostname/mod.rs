//! Gateway API Route hostname radix matching.
//!
//! Route matching follows Gateway API hostname-intersection semantics and may
//! match multiple labels below a wildcard suffix. Certificate selection is kept
//! in [`rfc6125`] and intentionally matches exactly one wildcard label.

mod input;
mod pattern;
mod radix;

pub use input::{normalize_route_sni, NormalizedRouteSni, RadixHostnameQuery};
pub use pattern::{radix_hostname_matches, RadixHostnamePatternError, RadixHostnamePatternRef, MAX_HOSTNAME_LEN};
pub use radix::RadixHostnameBuildError;

use self::radix::{HostPatternId, HostnameRadixTree};
use rpds::HashTrieMapSync;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RadixHostnamePattern(Box<str>);

impl RadixHostnamePattern {
    pub fn parse(pattern: &str) -> Result<Self, RadixHostnamePatternError> {
        RadixHostnamePatternRef::parse(pattern)?;
        Ok(Self(pattern.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_wildcard(&self) -> bool {
        self.0.starts_with("*.")
    }

    pub(super) fn suffix(&self) -> &str {
        self.0.strip_prefix("*.").unwrap_or(&self.0)
    }
}

pub enum RadixHostnameChange<T> {
    Upsert(RadixHostnamePattern, Arc<T>),
    Remove(RadixHostnamePattern),
}

pub struct RadixHostnameMatcher<T> {
    hosts: Arc<HostnameRadixTree>,
    contents: HashTrieMapSync<HostPatternId, Arc<T>>,
}

impl<T> Clone for RadixHostnameMatcher<T> {
    fn clone(&self) -> Self {
        Self {
            hosts: Arc::clone(&self.hosts),
            contents: self.contents.clone(),
        }
    }
}

impl<T> Default for RadixHostnameMatcher<T> {
    fn default() -> Self {
        Self {
            hosts: Arc::new(HostnameRadixTree::empty()),
            contents: HashTrieMapSync::new_sync(),
        }
    }
}

impl<T> RadixHostnameMatcher<T> {
    /// Callers aggregate competing resources into one opaque value per pattern.
    pub fn build(mut groups: HashMap<RadixHostnamePattern, Arc<T>>) -> Result<Self, RadixHostnameBuildError> {
        let mut patterns: Vec<_> = groups.keys().cloned().collect();
        patterns.sort();
        let hosts = Arc::new(HostnameRadixTree::build(patterns)?);
        let mut contents = HashTrieMapSync::new_sync();
        for (id, pattern) in hosts.patterns().iter().enumerate() {
            let value = groups
                .remove(pattern)
                .expect("patterns are exactly the unique group keys");
            let id = u32::try_from(id).expect("the frozen tree already checked every pattern ID fits u32");
            contents.insert_mut(HostPatternId(id), value);
        }
        Ok(Self { hosts, contents })
    }

    /// Ordered cold operations coalesce before testing final pattern occupancy.
    /// Existing keys only: share the whole tree and patch persistent map paths.
    pub fn with_changes(
        &self,
        changes: impl IntoIterator<Item = RadixHostnameChange<T>>,
    ) -> Result<Self, RadixHostnameBuildError> {
        let mut final_changes = HashMap::new();
        for change in changes {
            match change {
                RadixHostnameChange::Upsert(pattern, value) => {
                    final_changes.insert(pattern, Some(value));
                }
                RadixHostnameChange::Remove(pattern) => {
                    final_changes.insert(pattern, None);
                }
            }
        }
        let topology_changed = final_changes
            .iter()
            .any(|(pattern, value)| self.hosts.id_for_pattern(pattern).is_some() != value.is_some());
        if topology_changed {
            let mut groups: HashMap<_, _> = self
                .groups()
                .map(|(pattern, value)| (pattern.clone(), Arc::clone(value)))
                .collect();
            for (pattern, value) in final_changes {
                match value {
                    Some(value) => {
                        groups.insert(pattern, value);
                    }
                    None => {
                        groups.remove(&pattern);
                    }
                }
            }
            return Self::build(groups);
        }
        let mut next = self.clone();
        for (pattern, value) in final_changes {
            if let Some(value) = value {
                let id = self
                    .hosts
                    .id_for_pattern(&pattern)
                    .expect("unchanged topology guarantees every upsert key already exists");
                if !Arc::ptr_eq(self.content(id), &value) {
                    next.contents.insert_mut(id, value);
                }
            }
        }
        Ok(next)
    }

    /// Full payload replacement still shares the tree when keys agree. Its
    /// O(H) discovery cost is explicit; incremental owners use with_changes.
    pub fn with_groups(
        &self,
        mut groups: HashMap<RadixHostnamePattern, Arc<T>>,
    ) -> Result<Self, RadixHostnameBuildError> {
        if groups.len() != self.len() || !self.hosts.patterns().iter().all(|pattern| groups.contains_key(pattern)) {
            return Self::build(groups);
        }
        // Equal cardinality and complete membership prove the same topology.
        // Its ordered patterns already give the IDs; no delta coalescing or
        // repeated radix traversal is needed for a complete replacement input.
        let mut next = self.clone();
        for (id, pattern) in self.hosts.patterns().iter().enumerate() {
            let value = groups.remove(pattern).expect("the complete key set was checked");
            let id =
                HostPatternId(u32::try_from(id).expect("the frozen tree already checked every pattern ID fits u32"));
            if !Arc::ptr_eq(self.content(id), &value) {
                next.contents.insert_mut(id, value);
            }
        }
        Ok(next)
    }

    pub fn matching_values<'a>(&'a self, query: RadixHostnameQuery<'a>) -> impl Iterator<Item = &'a T> + 'a {
        self.hosts
            .matching_ids(query.as_str())
            .map(|id| self.content(id).as_ref())
    }

    /// Cold exact-pattern access for rebuilding a touched protocol group.
    pub fn get(&self, pattern: &RadixHostnamePattern) -> Option<&Arc<T>> {
        self.hosts.id_for_pattern(pattern).map(|id| self.content(id))
    }

    /// Cold enumeration; IDs stay private and cannot leak across generations.
    pub fn groups(&self) -> impl Iterator<Item = (&RadixHostnamePattern, &Arc<T>)> {
        self.hosts.patterns().iter().enumerate().map(|(id, pattern)| {
            let id = u32::try_from(id).expect("the frozen tree already checked every pattern ID fits u32");
            (pattern, self.content(HostPatternId(id)))
        })
    }

    pub fn len(&self) -> usize {
        self.hosts.patterns().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn content(&self, id: HostPatternId) -> &Arc<T> {
        // A missing ID is internal corruption, not no-match: skipping it would
        // incorrectly authorize fallback to a weaker candidate. Construction
        // and updates are private and preserve the complete tree/map pairing.
        self.contents
            .get(&id)
            .expect("every frozen tree terminal has same-generation contents")
    }

    /// Returns whether two snapshots share the same immutable radix topology.
    ///
    /// This is intended for cold-path diagnostics and structural-sharing tests;
    /// it does not compare payloads.
    pub fn shares_hosts(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.hosts, &other.hosts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    fn pattern(value: &str) -> RadixHostnamePattern {
        RadixHostnamePattern::parse(value).unwrap()
    }

    fn matcher(patterns: &[&str]) -> RadixHostnameMatcher<String> {
        RadixHostnameMatcher::build(
            patterns
                .iter()
                .map(|p| (pattern(p), Arc::new((*p).to_string())))
                .collect(),
        )
        .unwrap()
    }

    fn hits(index: &RadixHostnameMatcher<String>, query: &str) -> Vec<String> {
        index
            .matching_values(RadixHostnameQuery::new(query).unwrap())
            .cloned()
            .collect()
    }

    #[test]
    fn exact_wildcard_boundaries_and_ancestor_continuation() {
        let index = matcher(&[
            "api.sub.example.com",
            "*.example.com",
            "*.sub.example.com",
            "*.abcdefghijklmn.com",
        ]);
        assert_eq!(
            hits(&index, "foo.api.sub.example.com"),
            ["*.sub.example.com", "*.example.com"]
        );
        assert_eq!(
            hits(&index, "api.sub.example.com"),
            ["api.sub.example.com", "*.sub.example.com", "*.example.com"]
        );
        assert_eq!(
            hits(&index, "other.sub.example.com"),
            ["*.sub.example.com", "*.example.com"]
        );
        assert!(hits(&index, "example.com").is_empty());
        assert!(hits(&index, "badexample.com").is_empty());
        assert_eq!(hits(&matcher(&["grpc", "*.com"]), "grpc"), ["grpc"]);
        assert_eq!(hits(&matcher(&["*.com"]), "a.b.com"), ["*.com"]);
    }

    #[test]
    fn content_updates_share_tree_and_preserve_non_clone_payload_snapshots() {
        struct Payload(usize);
        let a = Arc::new(Payload(1));
        let b = Arc::new(Payload(2));
        let old = RadixHostnameMatcher::build(HashMap::from([
            (pattern("a.test"), a.clone()),
            (pattern("*.test"), b.clone()),
        ]))
        .unwrap();
        let updated = Arc::new(Payload(3));
        let new = old
            .with_changes([RadixHostnameChange::Upsert(pattern("a.test"), updated.clone())])
            .unwrap();
        assert!(new.shares_hosts(&old));
        assert!(Arc::ptr_eq(new.get(&pattern("*.test")).unwrap(), &b));
        assert!(Arc::ptr_eq(new.get(&pattern("a.test")).unwrap(), &updated));
        assert!(Arc::ptr_eq(old.get(&pattern("a.test")).unwrap(), &a));
        assert_eq!(
            old.matching_values(RadixHostnameQuery::new("a.test").unwrap())
                .next()
                .unwrap()
                .0,
            1
        );
        assert_eq!(
            new.matching_values(RadixHostnameQuery::new("a.test").unwrap())
                .next()
                .unwrap()
                .0,
            3
        );
        let empty_payload = RadixHostnameMatcher::build(HashMap::from([(pattern("test"), Arc::new(()))])).unwrap();
        assert_eq!(
            empty_payload
                .matching_values(RadixHostnameQuery::new("test").unwrap())
                .count(),
            1
        );
    }

    #[test]
    fn full_replacement_shares_tree_with_fresh_non_clone_contents() {
        struct Payload(usize);
        let old = RadixHostnameMatcher::build(HashMap::from([
            (pattern("a.test"), Arc::new(Payload(1))),
            (pattern("*.test"), Arc::new(Payload(2))),
        ]))
        .unwrap();
        let exact = Arc::new(Payload(3));
        let wildcard = Arc::new(Payload(4));
        let new = old
            .with_groups(HashMap::from([
                (pattern("*.test"), wildcard.clone()),
                (pattern("a.test"), exact.clone()),
            ]))
            .unwrap();
        assert!(new.shares_hosts(&old));
        assert!(Arc::ptr_eq(new.get(&pattern("a.test")).unwrap(), &exact));
        assert!(Arc::ptr_eq(new.get(&pattern("*.test")).unwrap(), &wildcard));
        let query = RadixHostnameQuery::new("a.test").unwrap();
        assert_eq!(
            old.matching_values(query).map(|value| value.0).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(
            new.matching_values(query).map(|value| value.0).collect::<Vec<_>>(),
            [3, 4]
        );
        let renamed = new
            .with_groups(HashMap::from([
                (pattern("z.test"), exact.clone()),
                (pattern("*.test"), wildcard.clone()),
            ]))
            .unwrap();
        assert!(!renamed.shares_hosts(&new));
        assert!(Arc::ptr_eq(renamed.get(&pattern("z.test")).unwrap(), &exact));
        assert!(renamed.get(&pattern("a.test")).is_none());
        assert!(Arc::ptr_eq(new.get(&pattern("a.test")).unwrap(), &exact));
    }

    #[test]
    fn topology_rebuild_remaps_ids_by_pattern_and_reuses_payloads() {
        let old = matcher(&["z.test", "*.test", "test"]);
        let held = old.get(&pattern("z.test")).unwrap().clone();
        let new = old
            .with_changes([
                RadixHostnameChange::Remove(pattern("*.test")),
                RadixHostnameChange::Upsert(pattern("a.test"), Arc::new("new".into())),
            ])
            .unwrap();
        assert!(!new.shares_hosts(&old));
        assert!(Arc::ptr_eq(new.get(&pattern("z.test")).unwrap(), &held));
        assert_eq!(hits(&old, "z.test"), ["z.test", "*.test"]);
        assert_eq!(hits(&new, "z.test"), ["z.test"]);
        assert_eq!(hits(&new, "test"), ["test"]);
        assert_eq!(hits(&new, "a.test"), ["new"]);
        assert_eq!(new.groups().count(), new.len());
    }

    #[test]
    fn exact_and_wildcard_terminals_at_one_suffix_are_independently_removed() {
        let both = matcher(&["example.com", "*.example.com"]);
        let without_exact = both
            .with_changes([RadixHostnameChange::Remove(pattern("example.com"))])
            .unwrap();
        assert!(hits(&without_exact, "example.com").is_empty());
        assert_eq!(hits(&without_exact, "a.example.com"), ["*.example.com"]);
        assert_eq!(hits(&both, "example.com"), ["example.com"]);

        let without_wildcard = both
            .with_changes([RadixHostnameChange::Remove(pattern("*.example.com"))])
            .unwrap();
        assert_eq!(hits(&without_wildcard, "example.com"), ["example.com"]);
        assert!(hits(&without_wildcard, "a.example.com").is_empty());
        assert_eq!(hits(&both, "a.example.com"), ["*.example.com"]);
    }

    #[test]
    fn coalesced_deltas_and_full_list_preserve_final_occupancy() {
        let old = matcher(&["a.test", "*.test"]);
        let new = old
            .with_changes([
                RadixHostnameChange::Remove(pattern("a.test")),
                RadixHostnameChange::Upsert(pattern("a.test"), Arc::new("replacement".into())),
                RadixHostnameChange::Upsert(pattern("gone.test"), Arc::new("temporary".into())),
                RadixHostnameChange::Remove(pattern("gone.test")),
            ])
            .unwrap();
        assert!(new.shares_hosts(&old));
        assert_eq!(hits(&new, "a.test"), ["replacement", "*.test"]);
        let full = new
            .with_groups(HashMap::from([
                (pattern("*.test"), Arc::new("wild".into())),
                (pattern("a.test"), Arc::new("exact".into())),
            ]))
            .unwrap();
        assert!(full.shares_hosts(&old));
        assert_eq!(hits(&full, "a.test"), ["exact", "wild"]);
        let empty = full.with_groups(HashMap::new()).unwrap();
        assert!(empty.is_empty());
        assert!(hits(&empty, "a.test").is_empty());
        assert!(!empty.shares_hosts(&full));
        assert!(empty.with_groups(HashMap::new()).unwrap().shares_hosts(&empty));
    }

    #[test]
    fn exhaustive_small_oracle_and_insertion_orders() {
        let names = ["a", "b", "a.a", "b.a", "a.b", "a.a.a", "b.a.a", "a.b.a", "b.b.a"];
        let patterns: Vec<_> = names
            .iter()
            .flat_map(|name| [name.to_string(), format!("*.{name}")])
            .collect();
        // Each subset independently exercises compressed edges and shared terminals.
        for mask in 0..256u16 {
            let mut selected: Vec<_> = patterns
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << (i % 8)) != 0)
                .map(|(_, p)| p.as_str())
                .collect();
            for reverse in [false, true] {
                if reverse {
                    selected.reverse();
                }
                let index = matcher(&selected);
                for name in names {
                    let mut expected: Vec<_> = selected
                        .iter()
                        .copied()
                        .filter(|p| {
                            if let Some(suffix) = p.strip_prefix("*.") {
                                name.strip_suffix(suffix)
                                    .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
                            } else {
                                *p == name
                            }
                        })
                        .collect();
                    expected.sort_by_key(|p| (p.starts_with("*."), std::cmp::Reverse(p.len())));
                    assert_eq!(hits(&index, name), expected, "mask={mask}, query={name}");
                }
            }
        }
    }

    #[test]
    fn maximum_depth_is_returned_by_the_lazy_iterator() {
        let query = vec!["a"; 127].join(".");
        assert_eq!(query.len(), 253);
        let mut patterns = vec![query.clone()];
        for labels in 1..127 {
            patterns.push(format!("*.{}", vec!["a"; labels].join(".")));
        }
        let index = matcher(&patterns.iter().map(String::as_str).collect::<Vec<_>>());
        let matches = hits(&index, &query);
        assert_eq!(matches.len(), 127);
        assert_eq!(matches[0], query);
        assert_eq!(matches.last().unwrap(), "*.a");
        assert!(size_of::<super::radix::MatchingIds<'_, '_>>() <= 40);
    }

    #[test]
    fn maximum_length_edges_and_wildcard_suffixes_are_not_truncated() {
        let exact = "a".repeat(253);
        let index = matcher(&[&exact]);
        assert_eq!(hits(&index, &exact), std::slice::from_ref(&exact));
        for position in [0, exact.len() / 2, exact.len() - 1] {
            let mut mismatch = exact.clone();
            mismatch.replace_range(position..position + 1, "b");
            assert!(hits(&index, &mismatch).is_empty(), "position={position}");
        }

        let suffix = "a".repeat(251);
        let wildcard = format!("*.{suffix}");
        let query = format!("b.{suffix}");
        assert_eq!(wildcard.len(), 253);
        assert_eq!(query.len(), 253);
        assert_eq!(hits(&matcher(&[&wildcard]), &query), [wildcard]);
    }

    #[test]
    fn long_common_edge_keeps_distinct_children() {
        let suffix = "a".repeat(250);
        let first = format!("b.{suffix}");
        let second = format!("c.{suffix}");
        let index = matcher(&[&first, &second]);
        assert_eq!(hits(&index, &first), [first]);
        assert_eq!(hits(&index, &second), [second]);
    }
}
