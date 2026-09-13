//! Frozen reverse-byte radix. Protocol contents never live in this tree.

use super::hostname::CompiledHostnamePattern;

/// A configured pattern's identity, private to one tree generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct HostPatternId(pub(super) usize);

// A 253-byte name has at most 127 nonempty labels: 126 wildcard ancestors
// plus its exact terminal. Checked queries cannot exceed this bound.
const MAX_MATCHES: usize = 127;

pub(super) struct HostnameRadixTree {
    root: Node,
    patterns: Box<[CompiledHostnamePattern]>,
}

struct Node {
    edge: Box<[u8]>,
    exact: Option<HostPatternId>,
    wildcard: Option<HostPatternId>,
    children: Box<[Node]>,
}

struct BuildEntry {
    reversed: Vec<u8>,
    id: HostPatternId,
    wildcard: bool,
}

impl HostnameRadixTree {
    pub(super) fn build(patterns: Vec<CompiledHostnamePattern>) -> Self {
        let mut entries: Vec<_> = patterns
            .iter()
            .enumerate()
            .map(|(id, pattern)| BuildEntry {
                reversed: pattern.suffix().bytes().rev().collect(),
                id: HostPatternId(id),
                wildcard: pattern.is_wildcard(),
            })
            .collect();
        entries.sort_by(|a, b| a.reversed.cmp(&b.reversed));
        Self {
            root: Node::build(&entries, 0),
            patterns: patterns.into_boxed_slice(),
        }
    }

    pub(super) fn patterns(&self) -> &[CompiledHostnamePattern] {
        &self.patterns
    }

    /// Cold exact CONFIGURED-pattern lookup, never wildcard query fallback.
    pub(super) fn id_for_pattern(&self, pattern: &CompiledHostnamePattern) -> Option<HostPatternId> {
        let query = pattern.suffix().as_bytes();
        let mut node = &self.root;
        let mut consumed = 0;
        loop {
            if !node.consume_edge(query, &mut consumed) {
                return None;
            }
            if consumed == query.len() {
                return if pattern.is_wildcard() {
                    node.wildcard
                } else {
                    node.exact
                };
            }
            node = node.child(query[query.len() - consumed - 1])?;
        }
    }

    pub(super) fn matching_ids(&self, query: &str) -> MatchingIds {
        let query = query.as_bytes();
        let mut matches = MatchingIds {
            ids: [HostPatternId(0); MAX_MATCHES],
            len: 0,
        };
        let mut node = &self.root;
        let mut consumed = 0;
        loop {
            if !node.consume_edge(query, &mut consumed) {
                break;
            }
            // The additional prefix must contain a nonempty label AND its dot.
            if query.len() > consumed + 1 && query[query.len() - consumed - 1] == b'.' {
                if let Some(id) = node.wildcard {
                    matches.push(id);
                }
            }
            if consumed == query.len() {
                if let Some(id) = node.exact {
                    matches.push(id);
                }
                break;
            }
            let Some(child) = node.child(query[query.len() - consumed - 1]) else {
                break;
            };
            node = child;
        }
        matches
    }
}

impl Node {
    fn build(entries: &[BuildEntry], depth: usize) -> Self {
        let Some(first) = entries.first() else {
            return Self {
                edge: Box::default(),
                exact: None,
                wildcard: None,
                children: Box::default(),
            };
        };
        // Sorted endpoints determine the common prefix of the entire range.
        let last = &entries[entries.len() - 1];
        let common = first.reversed[depth..]
            .iter()
            .zip(&last.reversed[depth..])
            .take_while(|(a, b)| a == b)
            .count();
        let end = depth + common;
        let mut node = Self {
            edge: first.reversed[depth..end].into(),
            exact: None,
            wildcard: None,
            children: Box::default(),
        };
        let mut offset = 0;
        while offset < entries.len() && entries[offset].reversed.len() == end {
            let entry = &entries[offset];
            if entry.wildcard {
                node.wildcard = Some(entry.id);
            } else {
                node.exact = Some(entry.id);
            }
            offset += 1;
        }
        let mut children = Vec::new();
        while offset < entries.len() {
            let start = offset;
            let next_byte = entries[start].reversed[end];
            while offset < entries.len()
                && entries[offset].reversed.len() > end
                && entries[offset].reversed[end] == next_byte
            {
                offset += 1;
            }
            children.push(Self::build(&entries[start..offset], end));
        }
        node.children = children.into_boxed_slice();
        node
    }

    fn consume_edge(&self, query: &[u8], consumed: &mut usize) -> bool {
        if query.len() - *consumed < self.edge.len() {
            return false;
        }
        for &byte in &self.edge {
            if byte != query[query.len() - *consumed - 1] {
                return false;
            }
            *consumed += 1;
        }
        true
    }

    fn child(&self, byte: u8) -> Option<&Self> {
        self.children
            .binary_search_by_key(&byte, |child| child.edge[0])
            .ok()
            .map(|index| &self.children[index])
    }
}

pub(super) struct MatchingIds {
    ids: [HostPatternId; MAX_MATCHES],
    len: usize,
}

impl MatchingIds {
    fn push(&mut self, id: HostPatternId) {
        // Private callers supply validated <=253-byte names; every wildcard
        // consumes a distinct label boundary. This is a structural bound.
        self.ids[self.len] = id;
        self.len += 1;
    }
}

impl Iterator for MatchingIds {
    type Item = HostPatternId;

    fn next(&mut self) -> Option<Self::Item> {
        self.len = self.len.checked_sub(1)?;
        Some(self.ids[self.len])
    }
}
