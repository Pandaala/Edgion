//! Build-time radix tree implementation
//!
//! This module provides the flexible tree structure used during route insertion.

use crate::core::matcher::radix_tree::error::RouterError;
use crate::core::matcher::radix_tree::frozen::FrozenRadixTree;
use std::collections::BTreeMap;

/// A radix tree node used during the build phase.
///
/// This structure is optimized for easy insertion and modification.
#[derive(Debug, Clone)]
pub(crate) struct BuildNode {
    /// The prefix string for this node
    prefix: Vec<u8>,

    /// Children nodes indexed by their first byte
    children: BTreeMap<u8, BuildNode>,

    /// Values stored at this node (supports multiple values per path)
    values: Vec<usize>,
}

impl BuildNode {
    fn new() -> Self {
        Self {
            prefix: Vec::new(),
            children: BTreeMap::new(),
            values: Vec::new(),
        }
    }

    fn with_prefix(prefix: Vec<u8>) -> Self {
        Self {
            prefix,
            children: BTreeMap::new(),
            values: Vec::new(),
        }
    }
}

/// A radix tree router for building routes.
///
/// This is the mutable builder used to insert routes. Once all routes are inserted,
/// call `freeze()` to convert it into a cache-friendly `FrozenRadixTree`.
#[derive(Debug)]
pub struct RadixRouter {
    root: BuildNode,
    route_count: usize,
}

impl RadixRouter {
    /// Creates a new empty router.
    pub fn new() -> Self {
        Self {
            root: BuildNode::new(),
            route_count: 0,
        }
    }

    /// Inserts a route with an associated value.
    ///
    /// # Arguments
    /// * `path` - The route path (e.g., "/api/users")
    /// * `value` - The value to associate with this route
    ///
    /// # Returns
    /// * `Ok(())` if insertion succeeded
    /// * `Err(RouterError)` if insertion fails due to constraints
    pub fn insert(&mut self, path: &str, value: usize) -> Result<(), RouterError> {
        let path_bytes = path.as_bytes();

        // Check path is not empty
        if path_bytes.is_empty() {
            return Err(RouterError::EmptyPath);
        }

        // Check path length doesn't exceed u16::MAX
        if path_bytes.len() > u16::MAX as usize {
            return Err(RouterError::PathTooLong {
                length: path_bytes.len(),
                max: u16::MAX as usize,
            });
        }

        Self::insert_helper(&mut self.root, path_bytes, value, path)?;
        self.route_count += 1;
        Ok(())
    }

    /// Helper function for recursive insertion
    fn insert_helper(node: &mut BuildNode, path: &[u8], value: usize, original_path: &str) -> Result<(), RouterError> {
        // Find common prefix length
        let common_len = node.prefix.iter().zip(path.iter()).take_while(|(a, b)| a == b).count();

        // Case 1: Node prefix is longer than common prefix
        // Need to split the node
        if common_len < node.prefix.len() {
            // Create a new child with the remaining part of current prefix
            let mut old_node = BuildNode::with_prefix(node.prefix[common_len..].to_vec());
            old_node.children = std::mem::take(&mut node.children);
            old_node.values = std::mem::take(&mut node.values);

            // Update current node
            node.prefix.truncate(common_len);
            let first_byte = old_node.prefix[0];
            node.children.insert(first_byte, old_node);
        }

        // Case 2: Path is exactly the node prefix
        if common_len == path.len() {
            // Check if adding this value would exceed u8::MAX
            if node.values.len() >= u8::MAX as usize {
                return Err(RouterError::TooManyValues {
                    path: original_path.to_string(),
                    count: node.values.len() + 1,
                    max: u8::MAX as usize,
                });
            }

            // Allow multiple values for the same path
            node.values.push(value);
            return Ok(());
        }

        // Case 3: Path continues beyond the node prefix
        let remaining = &path[common_len..];
        let first_byte = remaining[0];

        // Check if child exists
        if let Some(child) = node.children.get_mut(&first_byte) {
            Self::insert_helper(child, remaining, value, original_path)?;
        } else {
            // Check if adding a child would exceed u16::MAX
            if node.children.len() >= u16::MAX as usize {
                return Err(RouterError::TooManyChildren {
                    count: node.children.len() + 1,
                    max: u16::MAX as usize,
                });
            }

            // Create new child node
            let mut new_child = BuildNode::with_prefix(remaining.to_vec());
            new_child.values.push(value);
            node.children.insert(first_byte, new_child);
        }

        Ok(())
    }

    /// Returns the number of routes inserted.
    pub fn len(&self) -> usize {
        self.route_count
    }

    /// Returns whether the router is empty.
    pub fn is_empty(&self) -> bool {
        self.route_count == 0
    }

    /// Converts this builder into a cache-friendly frozen tree.
    ///
    /// After freezing, the tree can no longer be modified, but lookups
    /// will be much faster due to improved memory locality.
    ///
    /// # Errors
    /// Returns an error if the tree exceeds any size limits during flattening.
    pub fn freeze(self) -> Result<FrozenRadixTree, RouterError> {
        FrozenRadixTree::from_builder(self.root)
    }
}

impl Default for RadixRouter {
    fn default() -> Self {
        Self::new()
    }
}

// Internal access for frozen tree conversion
impl BuildNode {
    pub(crate) fn prefix(&self) -> &[u8] {
        &self.prefix
    }

    pub(crate) fn children(&self) -> &BTreeMap<u8, BuildNode> {
        &self.children
    }

    pub(crate) fn values(&self) -> &[usize] {
        &self.values
    }
}
