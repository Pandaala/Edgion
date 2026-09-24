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
#[path = "../../../tests/unit/matcher/radix_hostname/mod_test.rs"]
mod tests;
