//! Frozen reverse-byte radix. Protocol contents never live in this tree.

use super::RadixHostnamePattern;
use std::{error::Error, fmt};

const NONE: u32 = u32::MAX;
const LINEAR_CHILD_SEARCH_MAX: usize = 4;
const HAS_EXACT: u8 = 1;
const HAS_WILDCARD: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct HostPatternId(pub(super) u32);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FlatHostnameNode {
    edge_offset: u32,
    children_offset: u32,
    parent_index: u32,
    terminal_id: u32,
    suffix_len: u8,
    edge_len: u8,
    children_len: u8,
    terminal_flags: u8,
}

impl FlatHostnameNode {
    const EMPTY: Self = Self {
        edge_offset: 0,
        children_offset: 0,
        parent_index: NONE,
        terminal_id: NONE,
        suffix_len: 0,
        edge_len: 0,
        children_len: 0,
        terminal_flags: 0,
    };
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ChildEntry {
    node_index: u32,
    first_byte: u8,
}

pub(super) struct HostnameRadixTree {
    nodes: Box<[FlatHostnameNode]>,
    children: Box<[ChildEntry]>,
    edge_bytes: Box<[u8]>,
    patterns: Box<[RadixHostnamePattern]>,
}

struct BuildEntry {
    reversed: Vec<u8>,
    pattern: RadixHostnamePattern,
    id: HostPatternId,
    wildcard: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapacityKind {
    Patterns,
    Nodes,
    Children,
    EdgeBytes,
    EdgeLength,
    SuffixLength,
    ChildrenPerNode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadixHostnameBuildError {
    kind: CapacityKind,
    value: usize,
    maximum: usize,
}

impl RadixHostnameBuildError {
    fn capacity(kind: CapacityKind, value: usize, maximum: usize) -> Self {
        Self { kind, value, maximum }
    }
}

impl fmt::Display for RadixHostnameBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let field = match self.kind {
            CapacityKind::Patterns => "pattern count",
            CapacityKind::Nodes => "node count",
            CapacityKind::Children => "child-entry count",
            CapacityKind::EdgeBytes => "edge-byte pool size",
            CapacityKind::EdgeLength => "compressed edge length",
            CapacityKind::SuffixLength => "hostname suffix length",
            CapacityKind::ChildrenPerNode => "children per node",
        };
        write!(
            formatter,
            "hostname radix {field} {} exceeds the flat representation maximum {}; reduce the number or length of configured Route hostnames",
            self.value, self.maximum
        )
    }
}

impl Error for RadixHostnameBuildError {}

#[derive(Clone, Copy)]
struct FreezeLimits {
    patterns: usize,
    nodes: usize,
    children: usize,
    edge_bytes: usize,
    edge_len: usize,
    suffix_len: usize,
    children_per_node: usize,
}

impl FreezeLimits {
    const PRODUCTION: Self = Self {
        patterns: NONE as usize,
        nodes: NONE as usize,
        children: u32::MAX as usize,
        edge_bytes: u32::MAX as usize,
        edge_len: u8::MAX as usize,
        suffix_len: u8::MAX as usize,
        children_per_node: u8::MAX as usize,
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FreezeStats {
    nodes: usize,
    children: usize,
    edge_bytes: usize,
}

impl FreezeStats {
    fn add(&mut self, field: CapacityKind, value: usize, limit: usize) -> Result<(), RadixHostnameBuildError> {
        let target = match field {
            CapacityKind::Nodes => &mut self.nodes,
            CapacityKind::Children => &mut self.children,
            CapacityKind::EdgeBytes => &mut self.edge_bytes,
            _ => return Err(RadixHostnameBuildError::capacity(field, value, limit)),
        };
        *target = target
            .checked_add(value)
            .ok_or_else(|| RadixHostnameBuildError::capacity(field, usize::MAX, limit))?;
        if *target > limit {
            return Err(RadixHostnameBuildError::capacity(field, *target, limit));
        }
        Ok(())
    }
}

impl HostnameRadixTree {
    pub(super) fn empty() -> Self {
        Self {
            nodes: Box::new([FlatHostnameNode::EMPTY]),
            children: Box::default(),
            edge_bytes: Box::default(),
            patterns: Box::default(),
        }
    }

    pub(super) fn build(patterns: Vec<RadixHostnamePattern>) -> Result<Self, RadixHostnameBuildError> {
        Self::build_with_limits(patterns, FreezeLimits::PRODUCTION)
    }

    fn build_with_limits(
        patterns: Vec<RadixHostnamePattern>,
        limits: FreezeLimits,
    ) -> Result<Self, RadixHostnameBuildError> {
        if patterns.len() > limits.patterns {
            return Err(RadixHostnameBuildError::capacity(
                CapacityKind::Patterns,
                patterns.len(),
                limits.patterns,
            ));
        }
        let mut entries = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let wildcard = pattern.is_wildcard();
            entries.push(BuildEntry {
                reversed: pattern.suffix().bytes().rev().collect(),
                pattern,
                id: HostPatternId(0),
                wildcard,
            });
        }
        entries.sort_by(|a, b| a.reversed.cmp(&b.reversed).then_with(|| a.wildcard.cmp(&b.wildcard)));
        let entry_count = entries.len();
        for (id, entry) in entries.iter_mut().enumerate() {
            let id = u32::try_from(id)
                .map_err(|_| RadixHostnameBuildError::capacity(CapacityKind::Patterns, entry_count, limits.patterns))?;
            if id == NONE {
                return Err(RadixHostnameBuildError::capacity(
                    CapacityKind::Patterns,
                    entry_count,
                    limits.patterns,
                ));
            }
            entry.id = HostPatternId(id);
        }
        let mut stats = FreezeStats::default();
        plan_node(&entries, 0, limits, &mut stats)?;
        let mut builder = FlatTreeBuilder::with_capacity(stats);
        let root = builder.emit_node(&entries, 0, NONE, limits)?;
        if root != 0 || builder.stats() != stats {
            return Err(RadixHostnameBuildError::capacity(
                CapacityKind::Nodes,
                builder.nodes.len(),
                stats.nodes,
            ));
        }
        Ok(Self {
            nodes: builder.nodes.into_boxed_slice(),
            children: builder.children.into_boxed_slice(),
            edge_bytes: builder.edge_bytes.into_boxed_slice(),
            patterns: entries
                .into_iter()
                .map(|entry| entry.pattern)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        })
    }

    pub(super) fn patterns(&self) -> &[RadixHostnamePattern] {
        &self.patterns
    }

    pub(super) fn id_for_pattern(&self, pattern: &RadixHostnamePattern) -> Option<HostPatternId> {
        let query = pattern.suffix().as_bytes();
        let mut node_index = 0;
        let mut query_end = query.len();
        loop {
            let node = self.nodes.get(node_index as usize)?;
            query_end = self.consume_edge(node, query, query_end)?;
            if query_end == 0 {
                return terminal_id(node, pattern.is_wildcard());
            }
            node_index = self.child(node, query[query_end - 1])?;
        }
    }

    pub(super) fn matching_ids<'tree, 'query>(&'tree self, query: &'query str) -> MatchingIds<'tree, 'query> {
        let query = query.as_bytes();
        let mut node_index = 0;
        let mut query_end = query.len();
        let mut deepest = NONE;
        while let Some(node) = self.nodes.get(node_index as usize) {
            let Some(next_end) = self.consume_edge(node, query, query_end) else {
                break;
            };
            query_end = next_end;
            deepest = node_index;
            if query_end == 0 {
                break;
            }
            let Some(child) = self.child(node, query[query_end - 1]) else {
                break;
            };
            node_index = child;
        }
        let exact = if query_end == 0 && deepest != NONE {
            self.nodes
                .get(deepest as usize)
                .and_then(|node| terminal_id(node, false))
        } else {
            None
        };
        MatchingIds {
            tree: self,
            query,
            next_node: deepest,
            exact,
        }
    }

    #[inline]
    fn consume_edge(&self, node: &FlatHostnameNode, query: &[u8], query_end: usize) -> Option<usize> {
        let edge_len = node.edge_len as usize;
        let query_start = query_end.checked_sub(edge_len)?;
        let edge_start = node.edge_offset as usize;
        let edge_end = edge_start.checked_add(edge_len)?;
        (query.get(query_start..query_end)? == self.edge_bytes.get(edge_start..edge_end)?).then_some(query_start)
    }

    #[inline]
    fn child(&self, node: &FlatHostnameNode, byte: u8) -> Option<u32> {
        let start = node.children_offset as usize;
        let end = start.checked_add(node.children_len as usize)?;
        let children = self.children.get(start..end)?;
        let entry = if children.len() <= LINEAR_CHILD_SEARCH_MAX {
            children.iter().find(|entry| entry.first_byte == byte)
        } else {
            children
                .binary_search_by_key(&byte, |entry| entry.first_byte)
                .ok()
                .and_then(|index| children.get(index))
        }?;
        (entry.node_index != NONE).then_some(entry.node_index)
    }
}

fn common_end(entries: &[BuildEntry], depth: usize) -> usize {
    let Some(first) = entries.first() else {
        return depth;
    };
    let last = &entries[entries.len() - 1];
    depth
        + first.reversed[depth..]
            .iter()
            .zip(&last.reversed[depth..])
            .take_while(|(left, right)| left == right)
            .count()
}

fn terminal_end(entries: &[BuildEntry], end: usize) -> usize {
    entries.partition_point(|entry| entry.reversed.len() == end)
}

fn count_child_ranges(entries: &[BuildEntry], end: usize, mut offset: usize) -> usize {
    let mut count = 0;
    while offset < entries.len() {
        let next_byte = entries[offset].reversed[end];
        offset += 1;
        while offset < entries.len() && entries[offset].reversed[end] == next_byte {
            offset += 1;
        }
        count += 1;
    }
    count
}

fn plan_node(
    entries: &[BuildEntry],
    depth: usize,
    limits: FreezeLimits,
    stats: &mut FreezeStats,
) -> Result<(), RadixHostnameBuildError> {
    stats.add(CapacityKind::Nodes, 1, limits.nodes)?;
    let end = common_end(entries, depth);
    let edge_len = end - depth;
    if edge_len > limits.edge_len {
        return Err(RadixHostnameBuildError::capacity(
            CapacityKind::EdgeLength,
            edge_len,
            limits.edge_len,
        ));
    }
    if end > limits.suffix_len {
        return Err(RadixHostnameBuildError::capacity(
            CapacityKind::SuffixLength,
            end,
            limits.suffix_len,
        ));
    }
    stats.add(CapacityKind::EdgeBytes, edge_len, limits.edge_bytes)?;
    let terminal_end = terminal_end(entries, end);
    let child_count = count_child_ranges(entries, end, terminal_end);
    if child_count > limits.children_per_node {
        return Err(RadixHostnameBuildError::capacity(
            CapacityKind::ChildrenPerNode,
            child_count,
            limits.children_per_node,
        ));
    }
    stats.add(CapacityKind::Children, child_count, limits.children)?;
    let mut offset = terminal_end;
    while offset < entries.len() {
        let start = offset;
        let next_byte = entries[start].reversed[end];
        offset += 1;
        while offset < entries.len() && entries[offset].reversed[end] == next_byte {
            offset += 1;
        }
        plan_node(&entries[start..offset], end, limits, stats)?;
    }
    Ok(())
}

struct FlatTreeBuilder {
    nodes: Vec<FlatHostnameNode>,
    children: Vec<ChildEntry>,
    edge_bytes: Vec<u8>,
}

impl FlatTreeBuilder {
    fn with_capacity(stats: FreezeStats) -> Self {
        Self {
            nodes: Vec::with_capacity(stats.nodes),
            children: Vec::with_capacity(stats.children),
            edge_bytes: Vec::with_capacity(stats.edge_bytes),
        }
    }

    fn stats(&self) -> FreezeStats {
        FreezeStats {
            nodes: self.nodes.len(),
            children: self.children.len(),
            edge_bytes: self.edge_bytes.len(),
        }
    }

    fn emit_node(
        &mut self,
        entries: &[BuildEntry],
        depth: usize,
        parent_index: u32,
        limits: FreezeLimits,
    ) -> Result<u32, RadixHostnameBuildError> {
        let node_index = checked_index(self.nodes.len(), CapacityKind::Nodes, limits.nodes)?;
        self.nodes.push(FlatHostnameNode::EMPTY);
        let end = common_end(entries, depth);
        let edge_len = checked_u8(end - depth, CapacityKind::EdgeLength, limits.edge_len)?;
        let suffix_len = checked_u8(end, CapacityKind::SuffixLength, limits.suffix_len)?;
        let edge_offset = checked_offset(self.edge_bytes.len(), CapacityKind::EdgeBytes, limits.edge_bytes)?;
        if let Some(first) = entries.first() {
            self.edge_bytes.extend(first.reversed[depth..end].iter().rev());
        }
        if self.edge_bytes.len() > limits.edge_bytes {
            return Err(RadixHostnameBuildError::capacity(
                CapacityKind::EdgeBytes,
                self.edge_bytes.len(),
                limits.edge_bytes,
            ));
        }
        let terminal_end = terminal_end(entries, end);
        let mut terminal_id = NONE;
        let mut terminal_flags = 0;
        for entry in &entries[..terminal_end] {
            if terminal_id == NONE {
                terminal_id = entry.id.0;
            }
            if entry.wildcard {
                terminal_flags |= HAS_WILDCARD;
            } else {
                terminal_flags |= HAS_EXACT;
            }
        }
        if terminal_flags == HAS_EXACT | HAS_WILDCARD {
            let wildcard_id = terminal_id.checked_add(1).filter(|id| *id != NONE).ok_or_else(|| {
                RadixHostnameBuildError::capacity(CapacityKind::Patterns, terminal_id as usize + 2, limits.patterns)
            })?;
            if entries.first().is_none_or(|entry| entry.wildcard)
                || entries
                    .get(1)
                    .is_none_or(|entry| !entry.wildcard || entry.id.0 != wildcard_id)
            {
                return Err(RadixHostnameBuildError::capacity(
                    CapacityKind::Patterns,
                    terminal_id as usize + 2,
                    limits.patterns,
                ));
            }
        }
        let child_count = count_child_ranges(entries, end, terminal_end);
        let children_len = checked_u8(child_count, CapacityKind::ChildrenPerNode, limits.children_per_node)?;
        let children_offset = checked_offset(self.children.len(), CapacityKind::Children, limits.children)?;
        let child_end =
            self.children.len().checked_add(child_count).ok_or_else(|| {
                RadixHostnameBuildError::capacity(CapacityKind::Children, usize::MAX, limits.children)
            })?;
        if child_end > limits.children {
            return Err(RadixHostnameBuildError::capacity(
                CapacityKind::Children,
                child_end,
                limits.children,
            ));
        }
        self.children.resize(child_end, ChildEntry::default());
        self.nodes[node_index as usize] = FlatHostnameNode {
            edge_offset,
            children_offset,
            parent_index,
            terminal_id,
            suffix_len,
            edge_len,
            children_len,
            terminal_flags,
        };
        let mut offset = terminal_end;
        let mut child_slot = children_offset as usize;
        while offset < entries.len() {
            let start = offset;
            let first_byte = entries[start].reversed[end];
            offset += 1;
            while offset < entries.len() && entries[offset].reversed[end] == first_byte {
                offset += 1;
            }
            let child_index = self.emit_node(&entries[start..offset], end, node_index, limits)?;
            self.children[child_slot] = ChildEntry {
                node_index: child_index,
                first_byte,
            };
            child_slot += 1;
        }
        Ok(node_index)
    }
}

fn checked_index(value: usize, kind: CapacityKind, limit: usize) -> Result<u32, RadixHostnameBuildError> {
    if value >= limit {
        return Err(RadixHostnameBuildError::capacity(kind, value.saturating_add(1), limit));
    }
    u32::try_from(value).map_err(|_| RadixHostnameBuildError::capacity(kind, value, limit))
}

fn checked_offset(value: usize, kind: CapacityKind, limit: usize) -> Result<u32, RadixHostnameBuildError> {
    if value > limit {
        return Err(RadixHostnameBuildError::capacity(kind, value, limit));
    }
    u32::try_from(value).map_err(|_| RadixHostnameBuildError::capacity(kind, value, limit))
}

fn checked_u8(value: usize, kind: CapacityKind, limit: usize) -> Result<u8, RadixHostnameBuildError> {
    if value > limit {
        return Err(RadixHostnameBuildError::capacity(kind, value, limit));
    }
    u8::try_from(value).map_err(|_| RadixHostnameBuildError::capacity(kind, value, limit))
}

fn terminal_id(node: &FlatHostnameNode, wildcard: bool) -> Option<HostPatternId> {
    let required = if wildcard { HAS_WILDCARD } else { HAS_EXACT };
    if node.terminal_flags & required == 0 || node.terminal_id == NONE {
        return None;
    }
    let offset = u32::from(wildcard && node.terminal_flags & HAS_EXACT != 0);
    node.terminal_id
        .checked_add(offset)
        .filter(|id| *id != NONE)
        .map(HostPatternId)
}

pub(super) struct MatchingIds<'tree, 'query> {
    tree: &'tree HostnameRadixTree,
    query: &'query [u8],
    next_node: u32,
    exact: Option<HostPatternId>,
}

impl Iterator for MatchingIds<'_, '_> {
    type Item = HostPatternId;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(exact) = self.exact.take() {
            return Some(exact);
        }
        while self.next_node != NONE {
            let node = self.tree.nodes.get(self.next_node as usize)?;
            self.next_node = node.parent_index;
            let Some(wildcard_id) = terminal_id(node, true) else {
                continue;
            };
            let suffix_len = node.suffix_len as usize;
            if self.query.len() > suffix_len + 1 && self.query.get(self.query.len() - suffix_len - 1) == Some(&b'.') {
                return Some(wildcard_id);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    fn pattern(value: &str) -> RadixHostnamePattern {
        RadixHostnamePattern::parse(value).unwrap()
    }

    #[test]
    fn flat_layout_sizes_are_stable() {
        assert_eq!(size_of::<FlatHostnameNode>(), 20);
        assert_eq!(align_of::<FlatHostnameNode>(), 4);
        assert_eq!(size_of::<ChildEntry>(), 5);
        assert_eq!(align_of::<ChildEntry>(), 1);
        assert!(size_of::<MatchingIds<'_, '_>>() <= 40);
    }

    #[test]
    fn planned_and_emitted_pool_sizes_match() {
        let tree = HostnameRadixTree::build(vec![
            pattern("a.example.com"),
            pattern("*.example.com"),
            pattern("b.example.com"),
            pattern("grpc"),
        ])
        .unwrap();
        assert_eq!(tree.nodes.len(), tree.children.len() + 1);
        assert_eq!(tree.edge_bytes.len(), "a.example.combgrpc".len());
    }

    #[test]
    fn checked_planning_limits_reject_each_pool_without_allocating_it() {
        let patterns = vec![pattern("a.example.com"), pattern("b.example.com")];
        for (limits, kind) in [
            (
                FreezeLimits {
                    patterns: 1,
                    ..FreezeLimits::PRODUCTION
                },
                CapacityKind::Patterns,
            ),
            (
                FreezeLimits {
                    nodes: 1,
                    ..FreezeLimits::PRODUCTION
                },
                CapacityKind::Nodes,
            ),
            (
                FreezeLimits {
                    children: 1,
                    ..FreezeLimits::PRODUCTION
                },
                CapacityKind::Children,
            ),
            (
                FreezeLimits {
                    edge_bytes: 1,
                    ..FreezeLimits::PRODUCTION
                },
                CapacityKind::EdgeBytes,
            ),
            (
                FreezeLimits {
                    edge_len: 1,
                    ..FreezeLimits::PRODUCTION
                },
                CapacityKind::EdgeLength,
            ),
            (
                FreezeLimits {
                    suffix_len: 1,
                    ..FreezeLimits::PRODUCTION
                },
                CapacityKind::SuffixLength,
            ),
            (
                FreezeLimits {
                    children_per_node: 1,
                    ..FreezeLimits::PRODUCTION
                },
                CapacityKind::ChildrenPerNode,
            ),
        ] {
            let Err(error) = HostnameRadixTree::build_with_limits(patterns.clone(), limits) else {
                panic!("the injected capacity must reject this topology");
            };
            assert_eq!(error.kind, kind);
        }
    }

    #[test]
    fn exact_first_is_lazy_about_parent_wildcards() {
        let tree = HostnameRadixTree::build(vec![pattern("a.example.com"), pattern("*.example.com")]).unwrap();
        let mut ids = tree.matching_ids("a.example.com");
        let parent_before = ids.next_node;
        assert!(ids.next().is_some());
        assert_eq!(ids.next_node, parent_before);
        assert!(ids.next().is_some());
    }

    #[test]
    fn terminal_base_decodes_single_and_double_terminals_without_using_none() {
        let exact_only = FlatHostnameNode {
            terminal_id: NONE - 1,
            terminal_flags: HAS_EXACT,
            ..FlatHostnameNode::EMPTY
        };
        assert_eq!(terminal_id(&exact_only, false), Some(HostPatternId(NONE - 1)));
        assert_eq!(terminal_id(&exact_only, true), None);

        let wildcard_only = FlatHostnameNode {
            terminal_id: NONE - 1,
            terminal_flags: HAS_WILDCARD,
            ..FlatHostnameNode::EMPTY
        };
        assert_eq!(terminal_id(&wildcard_only, false), None);
        assert_eq!(terminal_id(&wildcard_only, true), Some(HostPatternId(NONE - 1)));

        let both = FlatHostnameNode {
            terminal_id: NONE - 2,
            terminal_flags: HAS_EXACT | HAS_WILDCARD,
            ..FlatHostnameNode::EMPTY
        };
        assert_eq!(terminal_id(&both, false), Some(HostPatternId(NONE - 2)));
        assert_eq!(terminal_id(&both, true), Some(HostPatternId(NONE - 1)));

        let invalid_both = FlatHostnameNode {
            terminal_id: NONE - 1,
            terminal_flags: HAS_EXACT | HAS_WILDCARD,
            ..FlatHostnameNode::EMPTY
        };
        assert_eq!(terminal_id(&invalid_both, true), None);
    }

    #[test]
    fn topology_and_pattern_ids_are_deterministic_across_input_orders() {
        let values = ["a.example.com", "*.example.com", "b.example.com", "grpc"];
        let forward = HostnameRadixTree::build(values.iter().map(|value| pattern(value)).collect()).unwrap();
        let reverse = HostnameRadixTree::build(values.iter().rev().map(|value| pattern(value)).collect()).unwrap();
        assert_eq!(forward.nodes.as_ref(), reverse.nodes.as_ref());
        assert_eq!(forward.children.as_ref(), reverse.children.as_ref());
        assert_eq!(forward.edge_bytes.as_ref(), reverse.edge_bytes.as_ref());
        assert_eq!(forward.patterns.as_ref(), reverse.patterns.as_ref());
    }
}
