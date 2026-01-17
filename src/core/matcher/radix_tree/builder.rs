//! Build-time radix tree implementation
//!
//! This module provides the flexible tree structure used during route insertion.
//!
//! # Parameter Support
//!
//! Routes can contain parameter segments using `:name` syntax:
//! - `/api/:version/users` - `:version` matches any non-empty segment
//! - `/api/::literal` - `::` escapes to literal `:`, matches `/api/:literal`
//!
//! Parameter segments match any content until the next `/` or end of path.

use crate::core::matcher::radix_tree::error::RouterError;
use crate::core::matcher::radix_tree::frozen::FrozenRadixTree;
use std::collections::BTreeMap;

/// Node type for distinguishing static vs parameter nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub(crate) enum NodeType {
    /// Static prefix match (e.g., "/api/users")
    #[default]
    Static = 0,
    /// Parameter match (e.g., ":id"), matches any non-empty segment until next '/'
    Param = 1,
}

/// A path segment parsed from the route pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PathSegment {
    /// Static segment (e.g., "/api/", "/users")
    Static(Vec<u8>),
    /// Parameter segment (matches any non-empty content until next '/')
    Param,
}

/// A radix tree node used during the build phase.
///
/// This structure is optimized for easy insertion and modification.
#[derive(Debug, Clone)]
pub(crate) struct BuildNode {
    /// The prefix string for this node (empty for Param nodes)
    prefix: Vec<u8>,

    /// Static children nodes indexed by their first byte
    children: BTreeMap<u8, BuildNode>,

    /// Parameter child node (at most one per node)
    param_child: Option<Box<BuildNode>>,

    /// Values stored at this node (supports multiple values per path)
    values: Vec<usize>,

    /// The type of this node
    node_type: NodeType,
}

impl BuildNode {
    fn new() -> Self {
        Self {
            prefix: Vec::new(),
            children: BTreeMap::new(),
            param_child: None,
            values: Vec::new(),
            node_type: NodeType::Static,
        }
    }

    fn with_prefix(prefix: Vec<u8>) -> Self {
        Self {
            prefix,
            children: BTreeMap::new(),
            param_child: None,
            values: Vec::new(),
            node_type: NodeType::Static,
        }
    }

    fn new_param() -> Self {
        Self {
            prefix: Vec::new(),
            children: BTreeMap::new(),
            param_child: None,
            values: Vec::new(),
            node_type: NodeType::Param,
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
    /// * `path` - The route path (e.g., "/api/users" or "/api/:version/users")
    /// * `value` - The value to associate with this route
    ///
    /// # Parameter Syntax
    /// - `:name` - Parameter segment, matches any non-empty content until next `/`
    /// - `::` - Escaped colon, treated as literal `:`
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

        // Parse path into segments
        let segments = Self::parse_path(path_bytes);

        // Insert segments into tree
        Self::insert_segments(&mut self.root, &segments, value, path)?;
        self.route_count += 1;
        Ok(())
    }

    /// Parse a path into segments, recognizing `:param` syntax.
    ///
    /// Rules:
    /// - `/:name` starts a parameter segment (`:` after `/`)
    /// - `::` is escaped to literal `:`
    /// - Parameter continues until next `/` or end of string
    fn parse_path(path: &[u8]) -> Vec<PathSegment> {
        let mut segments = Vec::new();
        let mut i = 0;
        let mut static_start = 0;

        while i < path.len() {
            // Check for "/:" pattern (parameter start)
            if path[i] == b'/' && i + 1 < path.len() && path[i + 1] == b':' {
                // Check for escape "/::" -> "/:"
                if i + 2 < path.len() && path[i + 2] == b':' {
                    // Escaped colon: collect static part including "/:"
                    // We need to build the static segment with the escape resolved
                    let mut static_part: Vec<u8> = path[static_start..=i].to_vec(); // include "/"
                    static_part.push(b':'); // add the escaped ":"

                    // Skip "/::" and collect remaining chars until next "/"
                    i += 3;

                    // Find next "/" or end to complete this static segment
                    while i < path.len() && path[i] != b'/' {
                        static_part.push(path[i]);
                        i += 1;
                    }

                    if !static_part.is_empty() {
                        segments.push(PathSegment::Static(static_part));
                    }
                    static_start = i;
                } else {
                    // Real parameter: "/:param"
                    // First, save any accumulated static part (including the "/")
                    if i > static_start {
                        segments.push(PathSegment::Static(path[static_start..=i].to_vec()));
                    } else if i == static_start {
                        // No prior static, but we still need the "/" before param
                        segments.push(PathSegment::Static(vec![b'/']));
                    }

                    // Skip "/:" and the parameter name
                    i += 2; // skip "/:"
                    while i < path.len() && path[i] != b'/' {
                        i += 1; // skip parameter name
                    }

                    // Add parameter segment
                    segments.push(PathSegment::Param);
                    static_start = i;
                }
            } else {
                i += 1;
            }
        }

        // Don't forget any trailing static content
        if static_start < path.len() {
            segments.push(PathSegment::Static(path[static_start..].to_vec()));
        }

        // Handle edge case: path starts with ":param" (no leading "/")
        // This shouldn't normally happen for valid HTTP paths, but let's handle it
        if segments.is_empty() && !path.is_empty() {
            segments.push(PathSegment::Static(path.to_vec()));
        }

        segments
    }

    /// Insert segments into the tree starting from the given node.
    fn insert_segments(
        node: &mut BuildNode,
        segments: &[PathSegment],
        value: usize,
        original_path: &str,
    ) -> Result<(), RouterError> {
        if segments.is_empty() {
            // No more segments, add value to current node
            if node.values.len() >= u8::MAX as usize {
                return Err(RouterError::TooManyValues {
                    path: original_path.to_string(),
                    count: node.values.len() + 1,
                    max: u8::MAX as usize,
                });
            }
            node.values.push(value);
            return Ok(());
        }

        match &segments[0] {
            PathSegment::Static(prefix) => {
                // For static segments, we need to find or create the right position
                // This is more complex because we need to handle prefix splitting
                Self::insert_static_segment(node, prefix, &segments[1..], value, original_path)
            }
            PathSegment::Param => {
                // Get or create the parameter child
                let param_child = node.param_child.get_or_insert_with(|| Box::new(BuildNode::new_param()));
                // Continue inserting remaining segments
                Self::insert_segments(param_child, &segments[1..], value, original_path)
            }
        }
    }

    /// Insert a static segment, handling prefix splitting as needed.
    fn insert_static_segment(
        node: &mut BuildNode,
        prefix: &[u8],
        remaining_segments: &[PathSegment],
        value: usize,
        original_path: &str,
    ) -> Result<(), RouterError> {
        // If this node is a Param node (empty prefix), go directly to children
        if node.node_type == NodeType::Param {
            let first_byte = prefix[0];
            if let Some(child) = node.children.get_mut(&first_byte) {
                return Self::insert_static_segment(child, prefix, remaining_segments, value, original_path);
            } else {
                // Create new static child
                if node.children.len() >= u16::MAX as usize {
                    return Err(RouterError::TooManyChildren {
                        count: node.children.len() + 1,
                        max: u16::MAX as usize,
                    });
                }
                let mut new_child = BuildNode::with_prefix(prefix.to_vec());
                Self::insert_segments(&mut new_child, remaining_segments, value, original_path)?;
                node.children.insert(first_byte, new_child);
                return Ok(());
            }
        }

        // Find common prefix length with current node
        let common_len = node
            .prefix
            .iter()
            .zip(prefix.iter())
            .take_while(|(a, b)| a == b)
            .count();

        // Case 1: Need to split the node
        if common_len < node.prefix.len() {
            let mut old_node = BuildNode::with_prefix(node.prefix[common_len..].to_vec());
            old_node.children = std::mem::take(&mut node.children);
            old_node.param_child = node.param_child.take();
            old_node.values = std::mem::take(&mut node.values);
            old_node.node_type = node.node_type;

            node.prefix.truncate(common_len);
            node.node_type = NodeType::Static;
            let first_byte = old_node.prefix[0];
            node.children.insert(first_byte, old_node);
        }

        // Case 2: Prefix fully consumed
        if common_len == prefix.len() {
            // Continue with remaining segments on current node
            return Self::insert_segments(node, remaining_segments, value, original_path);
        }

        // Case 3: Prefix continues beyond node prefix
        let remaining_prefix = &prefix[common_len..];
        let first_byte = remaining_prefix[0];

        if let Some(child) = node.children.get_mut(&first_byte) {
            Self::insert_static_segment(child, remaining_prefix, remaining_segments, value, original_path)
        } else {
            if node.children.len() >= u16::MAX as usize {
                return Err(RouterError::TooManyChildren {
                    count: node.children.len() + 1,
                    max: u16::MAX as usize,
                });
            }
            let mut new_child = BuildNode::with_prefix(remaining_prefix.to_vec());
            Self::insert_segments(&mut new_child, remaining_segments, value, original_path)?;
            node.children.insert(first_byte, new_child);
            Ok(())
        }
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

    pub(crate) fn param_child(&self) -> Option<&BuildNode> {
        self.param_child.as_deref()
    }

    pub(crate) fn values(&self) -> &[usize] {
        &self.values
    }

    pub(crate) fn node_type(&self) -> NodeType {
        self.node_type
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_path_static() {
        let segments = RadixRouter::parse_path(b"/api/users");
        assert_eq!(segments, vec![PathSegment::Static(b"/api/users".to_vec())]);
    }

    #[test]
    fn test_parse_path_single_param() {
        let segments = RadixRouter::parse_path(b"/api/:version/users");
        assert_eq!(
            segments,
            vec![
                PathSegment::Static(b"/api/".to_vec()),
                PathSegment::Param,
                PathSegment::Static(b"/users".to_vec()),
            ]
        );
    }

    #[test]
    fn test_parse_path_multi_params() {
        let segments = RadixRouter::parse_path(b"/api/:version/:resource");
        assert_eq!(
            segments,
            vec![
                PathSegment::Static(b"/api/".to_vec()),
                PathSegment::Param,
                PathSegment::Static(b"/".to_vec()),
                PathSegment::Param,
            ]
        );
    }

    #[test]
    fn test_parse_path_escaped_colon() {
        let segments = RadixRouter::parse_path(b"/api/::literal/test");
        assert_eq!(
            segments,
            vec![
                PathSegment::Static(b"/api/:literal".to_vec()),
                PathSegment::Static(b"/test".to_vec()),
            ]
        );
    }

    #[test]
    fn test_parse_path_param_at_end() {
        let segments = RadixRouter::parse_path(b"/users/:id");
        assert_eq!(
            segments,
            vec![PathSegment::Static(b"/users/".to_vec()), PathSegment::Param,]
        );
    }

    #[test]
    fn test_parse_path_root_only() {
        let segments = RadixRouter::parse_path(b"/");
        assert_eq!(segments, vec![PathSegment::Static(b"/".to_vec())]);
    }

    #[test]
    fn test_parse_path_param_empty_name() {
        // /api/:/test - param has empty name (just ":")
        let segments = RadixRouter::parse_path(b"/api/:/test");
        assert_eq!(
            segments,
            vec![
                PathSegment::Static(b"/api/".to_vec()),
                PathSegment::Param,
                PathSegment::Static(b"/test".to_vec()),
            ]
        );
    }

    #[test]
    fn test_parse_path_trailing_param() {
        // /api/: - param at end with empty name
        let segments = RadixRouter::parse_path(b"/api/:");
        assert_eq!(
            segments,
            vec![PathSegment::Static(b"/api/".to_vec()), PathSegment::Param,]
        );
    }

    #[test]
    fn test_parse_path_consecutive_escapes() {
        // /api/:::test -> /api/::test (first :: escapes, third : is literal)
        let segments = RadixRouter::parse_path(b"/api/:::test");
        assert_eq!(segments, vec![PathSegment::Static(b"/api/::test".to_vec())]);
    }

    #[test]
    fn test_parse_path_escape_then_param() {
        // /api/::x/:id -> Static("/api/:x"), Static("/"), Param
        let segments = RadixRouter::parse_path(b"/api/::x/:id");
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], PathSegment::Static(b"/api/:x".to_vec()));
        assert_eq!(segments[2], PathSegment::Param);
    }

    #[test]
    fn test_parse_path_double_slash() {
        // Double slash - treated as static
        let segments = RadixRouter::parse_path(b"/api//test");
        assert_eq!(segments, vec![PathSegment::Static(b"/api//test".to_vec())]);
    }
}
