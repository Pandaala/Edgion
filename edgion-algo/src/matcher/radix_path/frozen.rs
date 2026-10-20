//! Cache-friendly frozen path radix tree implementation
//!
//! This module provides the flattened, cache-optimized tree structure for fast lookups.
//! Supports parameter segments (`:param`) for dynamic route matching.

use super::builder::{BuildNode, NodeType};
use super::error::PathRadixError;
use smallvec::SmallVec;

/// Describes how the request path aligned with a matched tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMatchKind {
    /// The request path was fully consumed at this node.
    FullyConsumed,
    /// There is remaining path, and it starts on a segment boundary (`/`).
    SegmentBoundary,
    /// There is remaining path that does NOT start with `/` (partial segment).
    PartialSegment,
}

/// Sentinel value indicating no parameter child
const NO_PARAM_CHILD: u32 = u32::MAX;

/// A flattened node optimized for cache performance.
///
/// Supports multiple values per node with inline optimization:
/// - 0 values: values_count = 0
/// - 1-2 values: stored inline in values_data, values_flags = 0
/// - 3+ values: stored in values_pool, values_data[0] = offset, values_flags = 1
///
/// Supports parameter nodes for dynamic route matching.
///
/// Fields are carefully ordered to minimize padding while maintaining 8-byte alignment.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy)]
struct FlatNode {
    /// Offset in the string pool where this node's prefix starts
    prefix_offset: u32,

    /// Offset in the children array where this node's children start
    children_offset: u32,

    /// Values data: inline storage or offset
    /// - When inline (count <= 2): directly stores values
    /// - When external (count > 2): [offset_in_values_pool, _]
    values_data: [u32; 2],

    /// Index of parameter child node, or NO_PARAM_CHILD if none
    param_child_idx: u32,

    /// Length of the prefix
    prefix_len: u16,

    /// Number of children
    children_count: u16,

    /// Number of values stored at this node
    values_count: u8,

    /// Flags: bit 0 = 0 (inline), 1 (external in values_pool)
    values_flags: u8,

    /// Node type: 0 = Static, 1 = Param
    node_type: u8,

    /// Padding for alignment
    _padding: u8,
}

/// A child entry that maps a first byte to a node index.
///
/// Used for binary search during lookup.
///
/// Uses packed representation to eliminate padding, reducing size from 8 bytes to 5 bytes.
/// This provides 37.5% memory savings with minimal performance impact on modern CPUs.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct ChildEntry {
    /// The first byte of the child's prefix
    first_byte: u8,

    /// Index of the child node in the nodes array
    node_index: u32,
}

/// A cache-friendly, immutable radix tree for fast route matching.
///
/// This structure uses contiguous memory layout for optimal cache performance:
/// - All nodes are stored in a single Vec
/// - All child entries are in another Vec
/// - All string data is in a string pool
/// - All external values (3+) are in a values pool
///
/// This eliminates pointer chasing and improves spatial locality.
#[derive(Debug, Clone)]
pub struct PathRadixTree {
    /// Flat array of all nodes
    nodes: Vec<FlatNode>,

    /// Flat array of all child entries
    children: Vec<ChildEntry>,

    /// String pool containing all prefix strings
    string_pool: Vec<u8>,

    /// Values pool for nodes with 3+ values
    values_pool: Vec<u32>,
}

/// Internal iterator for matching prefixes in the tree.
/// This struct encapsulates the common logic for traversing the tree
/// and matching path prefixes, eliminating code duplication.
#[allow(dead_code)]
struct PrefixMatchIter<'a> {
    tree: &'a PathRadixTree,
    node_idx: usize,
    remaining: &'a [u8],
    finished: bool,
}

#[allow(dead_code)]
impl<'a> PrefixMatchIter<'a> {
    /// Creates a new prefix match iterator
    #[inline]
    fn new(tree: &'a PathRadixTree, path: &'a [u8]) -> Self {
        Self {
            tree,
            node_idx: 0,
            remaining: path,
            finished: tree.nodes.is_empty(),
        }
    }

    /// Returns the next matching node along the path.
    /// Returns Some((node_idx, has_values)) if a node matches, None if finished.
    fn next_match(&mut self) -> Option<(usize, bool)> {
        if self.finished {
            return None;
        }

        let node = &self.tree.nodes[self.node_idx];

        // Get the node's prefix from string pool
        let prefix = self.tree.get_node_prefix(node);

        // Check if prefix matches
        if !self.remaining.starts_with(prefix) {
            self.finished = true;
            return None;
        }

        let current_node_idx = self.node_idx;
        let has_values = node.values_count > 0;

        // Move past the matched prefix
        self.remaining = &self.remaining[prefix.len()..];

        // If we've consumed all the path, we're done
        if self.remaining.is_empty() {
            self.finished = true;
            return Some((current_node_idx, has_values));
        }

        // Look for a child that matches the next byte
        let next_byte = self.remaining[0];

        // Smart search in children (linear for small arrays, binary for large)
        let children_start = node.children_offset as usize;
        let children_end = children_start + node.children_count as usize;
        let children_slice = &self.tree.children[children_start..children_end];

        match self.tree.find_child(children_slice, next_byte) {
            Some(idx) => {
                // Found matching child, continue search
                self.node_idx = children_slice[idx].node_index as usize;
                Some((current_node_idx, has_values))
            }
            None => {
                // No matching child, finish iteration
                self.finished = true;
                Some((current_node_idx, has_values))
            }
        }
    }
}

impl PathRadixTree {
    /// Helper method to get the prefix bytes for a node
    #[inline]
    fn get_node_prefix(&self, node: &FlatNode) -> &[u8] {
        let prefix_start = node.prefix_offset as usize;
        let prefix_end = prefix_start + node.prefix_len as usize;
        &self.string_pool[prefix_start..prefix_end]
    }

    /// Helper method to get values for a node by index
    #[inline]
    fn get_node_values(&self, node_idx: usize) -> &[u32] {
        let node = &self.nodes[node_idx];
        if node.values_count == 0 {
            &[]
        } else if node.values_flags == 0 {
            // Inline storage
            &node.values_data[..node.values_count as usize]
        } else {
            // External storage
            let offset = node.values_data[0] as usize;
            let count = node.values_count as usize;
            &self.values_pool[offset..offset + count]
        }
    }

    /// Smart child search: uses linear search for small arrays (≤4 elements),
    /// binary search for larger arrays. Linear search is faster for small arrays
    /// due to better cache locality and branch prediction.
    #[inline]
    #[allow(dead_code)]
    fn find_child(&self, children: &[ChildEntry], byte: u8) -> Option<usize> {
        if children.len() <= 4 {
            // Linear search for small arrays - better cache and branch prediction
            for (i, child) in children.iter().enumerate() {
                if child.first_byte == byte {
                    return Some(i);
                }
            }
            None
        } else {
            // Binary search for larger arrays
            children.binary_search_by_key(&byte, |c| c.first_byte).ok()
        }
    }

    /// Creates a frozen tree from a build tree.
    pub(crate) fn from_builder(root: BuildNode) -> Result<Self, PathRadixError> {
        // Pre-calculate exact capacities to avoid reallocation
        let stats = count_tree_stats(&root);

        // Check node count doesn't exceed u32::MAX
        if stats.node_count > u32::MAX as usize {
            return Err(PathRadixError::TooManyNodes {
                count: stats.node_count,
                max: u32::MAX as usize,
            });
        }

        // Check string pool size doesn't exceed u32::MAX
        if stats.total_string_bytes > u32::MAX as usize {
            return Err(PathRadixError::StringPoolTooLarge {
                size: stats.total_string_bytes,
                max: u32::MAX as usize,
            });
        }

        // Check values pool size doesn't exceed u32::MAX
        if stats.total_values_external > u32::MAX as usize {
            return Err(PathRadixError::ValuesPoolTooLarge {
                size: stats.total_values_external,
                max: u32::MAX as usize,
            });
        }

        let mut builder = FlatTreeBuilder::with_capacity(
            stats.node_count,
            stats.child_count,
            stats.total_string_bytes,
            stats.total_values_external,
        );

        builder.flatten_node(&root)?;
        Ok(builder.build())
    }

    /// Matches a route and returns all values for the longest matching prefix.
    ///
    /// # Arguments
    /// * `path` - The path to match
    ///
    /// # Returns
    /// A slice of all values for the longest matching route (empty if no match)
    ///
    #[allow(dead_code)]
    pub fn match_route_longest(&self, path: &str) -> &[u32] {
        let mut iter = PrefixMatchIter::new(self, path.as_bytes());
        let mut last_match = None;

        while let Some((node_idx, has_values)) = iter.next_match() {
            if has_values {
                last_match = Some(node_idx);
            }
        }

        last_match.map_or(&[], |idx| self.get_node_values(idx))
    }

    /// Matches a route and returns ALL matching prefix values.
    ///
    /// Unlike `match_route()` which returns only the longest match,
    /// this returns all prefixes that match, in order from shortest to longest.
    ///
    /// # Arguments
    /// * `path` - The path to match
    ///
    /// # Returns
    /// A vector of all matching values, ordered from shortest to longest prefix
    ///
    #[allow(dead_code)]
    pub fn match_all_prefixes(&self, path: &str) -> SmallVec<[u32; 8]> {
        let mut iter = PrefixMatchIter::new(self, path.as_bytes());
        let mut results = SmallVec::<[u32; 8]>::new();

        while let Some((node_idx, has_values)) = iter.next_match() {
            if has_values {
                results.extend_from_slice(self.get_node_values(node_idx));
            }
        }

        results
    }

    /// Matches all routes (static, prefix, and parametric) that match the given path.
    ///
    /// This method performs a DFS traversal of the tree, exploring both static
    /// children and parameter children at each node. It returns all values from
    /// all matching routes.
    ///
    /// # Arguments
    /// * `path` - The path to match
    ///
    /// # Returns
    /// A vector of all matching values from all matching routes
    ///
    #[allow(dead_code)]
    pub fn match_all(&self, path: &str) -> SmallVec<[u32; 8]> {
        let mut results = SmallVec::<[u32; 8]>::new();

        if self.nodes.is_empty() {
            return results;
        }

        // Stack for DFS traversal: (node_index, remaining_path)
        let mut stack = SmallVec::<[(usize, &[u8]); 16]>::new();
        stack.push((0, path.as_bytes()));

        while let Some((node_idx, remaining)) = stack.pop() {
            let node = &self.nodes[node_idx];

            // Handle node based on its type
            let after_match = if node.node_type == NodeType::Param as u8 {
                // Parameter node: consume until next '/' or end
                let end = remaining.iter().position(|&c| c == b'/').unwrap_or(remaining.len());

                // Empty parameter doesn't match
                if end == 0 {
                    continue;
                }

                &remaining[end..]
            } else {
                // Static node: must match prefix exactly
                let prefix = self.get_node_prefix(node);

                if !remaining.starts_with(prefix) {
                    continue;
                }

                &remaining[prefix.len()..]
            };

            // Collect values from this node
            if node.values_count > 0 {
                results.extend_from_slice(self.get_node_values(node_idx));
            }

            // If there's more path to match, explore children
            if !after_match.is_empty() {
                let next_byte = after_match[0];

                // Try static children
                let children_start = node.children_offset as usize;
                let children_end = children_start + node.children_count as usize;

                for child_entry in &self.children[children_start..children_end] {
                    if child_entry.first_byte == next_byte {
                        stack.push((child_entry.node_index as usize, after_match));
                    }
                }

                // Try parameter child
                if node.param_child_idx != NO_PARAM_CHILD {
                    stack.push((node.param_child_idx as usize, after_match));
                }
            }
        }

        results
    }

    /// Like [`match_all`](Self::match_all), but each returned value carries a
    /// [`PathMatchKind`] that tells the caller whether the request path was fully
    /// consumed, ended on a segment boundary, or split a segment.
    ///
    /// This lets the caller distinguish exact vs prefix semantics in one pass
    /// without re-parsing the path.
    pub fn match_all_ext(&self, path: &str) -> SmallVec<[(u32, PathMatchKind); 8]> {
        let mut results = SmallVec::<[(u32, PathMatchKind); 8]>::new();
        self.for_each_match_ext(path, |value, kind| results.push((value, kind)));
        results
    }

    /// Visit every matching value and its boundary kind in traversal order.
    ///
    /// Uses the same traversal as [`Self::match_all_ext`] without collecting an
    /// intermediate result buffer. The traversal stack may still spill to heap.
    pub fn for_each_match_ext(&self, path: &str, mut visit: impl FnMut(u32, PathMatchKind)) {
        if self.nodes.is_empty() {
            return;
        }

        let mut stack = SmallVec::<[(usize, &[u8]); 16]>::new();
        stack.push((0, path.as_bytes()));

        while let Some((node_idx, remaining)) = stack.pop() {
            let node = &self.nodes[node_idx];

            let (after_match, prefix_ends_with_slash) = if node.node_type == NodeType::Param as u8 {
                let end = remaining.iter().position(|&c| c == b'/').unwrap_or(remaining.len());
                if end == 0 {
                    continue;
                }
                (&remaining[end..], false)
            } else {
                let prefix = self.get_node_prefix(node);
                if !remaining.starts_with(prefix) {
                    continue;
                }
                let ends_slash = !prefix.is_empty() && prefix[prefix.len() - 1] == b'/';
                (&remaining[prefix.len()..], ends_slash)
            };

            if node.values_count > 0 {
                let kind = if after_match.is_empty() {
                    PathMatchKind::FullyConsumed
                } else if after_match[0] == b'/' || prefix_ends_with_slash {
                    PathMatchKind::SegmentBoundary
                } else {
                    PathMatchKind::PartialSegment
                };
                for &v in self.get_node_values(node_idx) {
                    visit(v, kind);
                }
            }

            if !after_match.is_empty() {
                let next_byte = after_match[0];

                let children_start = node.children_offset as usize;
                let children_end = children_start + node.children_count as usize;

                for child_entry in &self.children[children_start..children_end] {
                    if child_entry.first_byte == next_byte {
                        stack.push((child_entry.node_index as usize, after_match));
                    }
                }

                if node.param_child_idx != NO_PARAM_CHILD {
                    stack.push((node.param_child_idx as usize, after_match));
                }
            }
        }
    }
}

/// Statistics for pre-calculating tree size
struct TreeStatistics {
    node_count: usize,
    child_count: usize,
    total_string_bytes: usize,
    total_values_external: usize, // Values that need external storage (count > 2)
}

/// Recursively count nodes, children, string bytes, and external values in the build tree
fn count_tree_stats(node: &BuildNode) -> TreeStatistics {
    let values_count = node.values().len();
    let external_values = if values_count > 2 { values_count } else { 0 };

    let mut stats = TreeStatistics {
        node_count: 1,
        child_count: node.children().len(),
        total_string_bytes: node.prefix().len(),
        total_values_external: external_values,
    };

    // Count static children
    for child in node.children().values() {
        let child_stats = count_tree_stats(child);
        stats.node_count += child_stats.node_count;
        stats.child_count += child_stats.child_count;
        stats.total_string_bytes += child_stats.total_string_bytes;
        stats.total_values_external += child_stats.total_values_external;
    }

    // Count parameter child if present
    if let Some(param_child) = node.param_child() {
        let param_stats = count_tree_stats(param_child);
        stats.node_count += param_stats.node_count;
        stats.child_count += param_stats.child_count;
        stats.total_string_bytes += param_stats.total_string_bytes;
        stats.total_values_external += param_stats.total_values_external;
    }

    stats
}

/// Helper for building a flat tree from a build tree.
struct FlatTreeBuilder {
    nodes: Vec<FlatNode>,
    children: Vec<ChildEntry>,
    string_pool: Vec<u8>,
    values_pool: Vec<u32>,
}

impl FlatTreeBuilder {
    fn with_capacity(nodes_cap: usize, children_cap: usize, string_cap: usize, values_cap: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(nodes_cap),
            children: Vec::with_capacity(children_cap),
            string_pool: Vec::with_capacity(string_cap),
            values_pool: Vec::with_capacity(values_cap),
        }
    }

    /// Recursively flattens a build node and its children.
    /// Returns the index of the flattened node.
    fn flatten_node(&mut self, node: &BuildNode) -> Result<u32, PathRadixError> {
        // Check string pool won't overflow
        let new_string_pool_size = self.string_pool.len() + node.prefix().len();
        if new_string_pool_size > u32::MAX as usize {
            return Err(PathRadixError::StringPoolTooLarge {
                size: new_string_pool_size,
                max: u32::MAX as usize,
            });
        }

        // Allocate space for prefix in string pool
        let prefix_offset = self.string_pool.len() as u32;
        self.string_pool.extend_from_slice(node.prefix());

        // Process values: inline or external storage
        let values = node.values();
        let values_count = values.len();

        // Check values count fits in u8
        if values_count > u8::MAX as usize {
            return Err(PathRadixError::TooManyValues {
                path: String::from_utf8_lossy(node.prefix()).to_string(),
                count: values_count,
                max: u8::MAX as usize,
            });
        }

        let (values_flags, values_data) = if values.is_empty() {
            // No values
            (0, [0, 0])
        } else if values.len() <= 2 {
            // Inline storage (1-2 values)
            let mut data = [0u32; 2];
            for (i, &v) in values.iter().enumerate() {
                data[i] = v as u32;
            }
            (0, data)
        } else {
            // External storage (3+ values)
            let new_values_pool_size = self.values_pool.len() + values.len();
            if new_values_pool_size > u32::MAX as usize {
                return Err(PathRadixError::ValuesPoolTooLarge {
                    size: new_values_pool_size,
                    max: u32::MAX as usize,
                });
            }

            let offset = self.values_pool.len() as u32;
            for &v in values {
                self.values_pool.push(v as u32);
            }
            (1, [offset, 0])
        };

        // Check prefix length fits in u16
        let prefix_len = node.prefix().len();
        if prefix_len > u16::MAX as usize {
            return Err(PathRadixError::PathTooLong {
                length: prefix_len,
                max: u16::MAX as usize,
            });
        }

        // Check children count fits in u16
        let children_count = node.children().len();
        if children_count > u16::MAX as usize {
            return Err(PathRadixError::TooManyChildren {
                count: children_count,
                max: u16::MAX as usize,
            });
        }

        // Check node index won't overflow
        if self.nodes.len() >= u32::MAX as usize {
            return Err(PathRadixError::TooManyNodes {
                count: self.nodes.len() + 1,
                max: u32::MAX as usize,
            });
        }

        // Create the flat node (without children info yet)
        let node_index = self.nodes.len() as u32;
        let flat_node = FlatNode {
            prefix_offset,
            children_offset: 0, // Will be updated later
            values_data,
            param_child_idx: NO_PARAM_CHILD, // Will be updated if param child exists
            prefix_len: prefix_len as u16,
            children_count: children_count as u16,
            values_count: values_count as u8,
            values_flags,
            node_type: node.node_type() as u8,
            _padding: 0,
        };

        self.nodes.push(flat_node);

        // First, recursively flatten all static children nodes
        let mut child_indices = Vec::with_capacity(node.children().len());
        for (&first_byte, child) in node.children().iter() {
            let child_index = self.flatten_node(child)?;
            child_indices.push((first_byte, child_index));
        }

        // Flatten parameter child if present
        let param_child_idx = if let Some(param_child) = node.param_child() {
            self.flatten_node(param_child)?
        } else {
            NO_PARAM_CHILD
        };
        self.nodes[node_index as usize].param_child_idx = param_child_idx;

        // Check children offset won't overflow
        if self.children.len() > u32::MAX as usize {
            return Err(PathRadixError::TooManyChildren {
                count: self.children.len(),
                max: u32::MAX as usize,
            });
        }

        // NOW set the children_offset (after all recursive calls are done)
        let children_offset = self.children.len() as u32;
        self.nodes[node_index as usize].children_offset = children_offset;

        // Add child entries for this node
        for (first_byte, child_index) in child_indices {
            self.children.push(ChildEntry {
                first_byte,
                node_index: child_index,
            });
        }

        Ok(node_index)
    }

    fn build(self) -> PathRadixTree {
        PathRadixTree {
            nodes: self.nodes,
            children: self.children,
            string_pool: self.string_pool,
            values_pool: self.values_pool,
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/matcher/radix_path/frozen_test.rs"]
mod tests;
