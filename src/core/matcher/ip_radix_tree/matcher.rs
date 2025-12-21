//! Cache-friendly frozen IPv6 radix tree implementation
//!
//! This module provides the flattened, cache-optimized tree structure for fast IPv6 lookups.

use super::builder_v6::IpV6BuildNode;
use super::error::IpRadixError;
use std::net::Ipv6Addr;

/// A flattened node optimized for cache performance
///
/// Binary radix tree node with two children (left=0, right=1).
/// Identical structure to IPv4, but operates on 128-bit addresses.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct FlatIpV6Node {
    /// Index of left child in nodes array (0 if none)
    pub(crate) left_index: u32,

    /// Index of right child in nodes array (0 if none)
    pub(crate) right_index: u32,

    /// Prefix length at this node (0 if not a terminal)
    pub(crate) prefix_len: u8,

    /// Flags: bit 0 = has_value, bit 1 = value (0=deny, 1=allow)
    pub(crate) flags: u8,

    /// Padding for alignment
    _padding: [u8; 2],
}

impl FlatIpV6Node {
    /// Check if this node has a value
    #[inline]
    fn has_value(&self) -> bool {
        (self.flags & 0x01) != 0
    }

    /// Get the value (true=allow, false=deny)
    /// Only valid if has_value() returns true
    #[inline]
    fn get_value(&self) -> bool {
        (self.flags & 0x02) != 0
    }
}

/// A cache-friendly, immutable IPv6 radix tree for fast IP matching
///
/// This structure uses contiguous memory layout for optimal cache performance.
/// All nodes are stored in a single Vec, eliminating pointer chasing.
#[derive(Debug, Clone)]
pub struct FrozenIpV6RadixTree {
    /// Flat array of all nodes (root is always at index 0)
    nodes: Vec<FlatIpV6Node>,
}

impl FrozenIpV6RadixTree {
    /// Creates a frozen tree from a build tree
    pub(crate) fn from_builder(root: IpV6BuildNode) -> Result<Self, IpRadixError> {
        // Pre-calculate node count
        let node_count = count_nodes(&root);

        // Check node count doesn't exceed u32::MAX
        if node_count > u32::MAX as usize {
            return Err(IpRadixError::TooManyNodes {
                count: node_count,
                max: u32::MAX as usize,
            });
        }

        let mut builder = FlatTreeBuilder::with_capacity(node_count);
        builder.flatten_node(&root)?;
        Ok(builder.build())
    }

    /// Matches an IPv6 address and returns the matching rule (if any)
    ///
    /// Uses longest prefix matching: returns the most specific rule that matches.
    ///
    /// # Arguments
    /// * `ip` - IPv6 address as u128 (network byte order)
    ///
    /// # Returns
    /// * `Some(true)` - IP is explicitly allowed
    /// * `Some(false)` - IP is explicitly denied
    /// * `None` - No matching rule
    ///
    /// # Examples
    /// ```
    /// use edgion::core::routes::http_routes::ip_radix_match::builder_v6::IpV6RadixBuilder;
    /// use std::net::Ipv6Addr;
    ///
    /// let mut builder = IpV6RadixBuilder::new();
    /// builder.insert("2001:db8::/32", true).unwrap();
    /// builder.insert("2001:db8::1/128", false).unwrap();
    ///
    /// let tree = builder.freeze().unwrap();
    ///
    /// let ip1: u128 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2).into();
    /// assert_eq!(tree.match_ip(ip1), Some(true));  // Matched by /32
    ///
    /// let ip2: u128 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1).into();
    /// assert_eq!(tree.match_ip(ip2), Some(false)); // Matched by /128 (more specific)
    ///
    /// let ip3: u128 = Ipv6Addr::new(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1).into();
    /// assert_eq!(tree.match_ip(ip3), None);        // No match
    /// ```
    pub fn match_ip(&self, ip: u128) -> Option<bool> {
        if self.nodes.is_empty() {
            return None;
        }

        let mut node_idx = 0; // Start at root
        let mut last_match: Option<bool> = None;
        let mut current_bit = 0;

        loop {
            let node = &self.nodes[node_idx];

            // If this node has a value, record it (longest prefix so far)
            if node.has_value() {
                last_match = Some(node.get_value());
            }

            // Check if we've consumed all 128 bits
            if current_bit >= 128 {
                break;
            }

            // Get next bit (from left/MSB to right/LSB)
            let bit = (ip >> (127 - current_bit)) & 1;

            // Traverse to child based on bit value
            let next_idx = if bit == 0 {
                node.left_index
            } else {
                node.right_index
            };

            if next_idx == 0 {
                // No child, stop here
                break;
            }

            node_idx = next_idx as usize;
            current_bit += 1;
        }

        last_match
    }

    /// Convenience method to match an Ipv6Addr
    pub fn match_ipv6(&self, ip: Ipv6Addr) -> Option<bool> {
        self.match_ip(u128::from(ip))
    }

    /// Returns the number of nodes in the tree
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns statistics about the frozen tree
    pub fn stats(&self) -> TreeStats {
        TreeStats {
            node_count: self.nodes.len(),
            total_bytes: self.nodes.len() * std::mem::size_of::<FlatIpV6Node>(),
        }
    }
}

/// Statistics about a frozen tree
#[derive(Debug, Clone, Copy)]
pub struct TreeStats {
    /// Number of nodes in the tree
    pub node_count: usize,

    /// Total memory usage in bytes
    pub total_bytes: usize,
}

/// Recursively count nodes in the build tree
fn count_nodes(node: &IpV6BuildNode) -> usize {
    let mut count = 1;

    if let Some(ref left) = node.left {
        count += count_nodes(left);
    }

    if let Some(ref right) = node.right {
        count += count_nodes(right);
    }

    count
}

/// Helper for building a flat tree from a build tree
struct FlatTreeBuilder {
    nodes: Vec<FlatIpV6Node>,
}

impl FlatTreeBuilder {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
        }
    }

    /// Recursively flattens a build node and its children
    /// Returns the index of the flattened node
    fn flatten_node(&mut self, node: &IpV6BuildNode) -> Result<u32, IpRadixError> {
        // Check node index won't overflow
        if self.nodes.len() >= u32::MAX as usize {
            return Err(IpRadixError::TooManyNodes {
                count: self.nodes.len() + 1,
                max: u32::MAX as usize,
            });
        }

        // Reserve space for this node
        let node_idx = self.nodes.len();

        // Create temporary node (will be updated with child indices)
        self.nodes.push(FlatIpV6Node {
            left_index: 0,
            right_index: 0,
            prefix_len: node.prefix_len,
            flags: encode_flags(node.value),
            _padding: [0; 2],
        });

        // Recursively flatten children
        let left_idx = if let Some(ref left) = node.left {
            self.flatten_node(left)?
        } else {
            0
        };

        let right_idx = if let Some(ref right) = node.right {
            self.flatten_node(right)?
        } else {
            0
        };

        // Update node with child indices
        self.nodes[node_idx].left_index = left_idx;
        self.nodes[node_idx].right_index = right_idx;

        Ok(node_idx as u32)
    }

    fn build(self) -> FrozenIpV6RadixTree {
        FrozenIpV6RadixTree { nodes: self.nodes }
    }
}

/// Encode value into flags byte
/// Bit 0: has_value (1 if Some, 0 if None)
/// Bit 1: value (1 if true/allow, 0 if false/deny)
fn encode_flags(value: Option<bool>) -> u8 {
    match value {
        Some(true) => 0x03,  // has_value=1, value=1
        Some(false) => 0x01, // has_value=1, value=0
        None => 0x00,        // has_value=0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::routes::http_routes::ip_radix_match::builder_v6::IpV6RadixBuilder;

    #[test]
    fn test_encode_flags() {
        assert_eq!(encode_flags(Some(true)), 0x03);
        assert_eq!(encode_flags(Some(false)), 0x01);
        assert_eq!(encode_flags(None), 0x00);
    }

    #[test]
    fn test_basic_match() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("2001:db8::/32", true).unwrap();
        let tree = builder.freeze().unwrap();

        let ip_match: u128 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1).into();
        let ip_no_match: u128 = Ipv6Addr::new(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1).into();

        assert_eq!(tree.match_ip(ip_match), Some(true));
        assert_eq!(tree.match_ip(ip_no_match), None);
    }

    #[test]
    fn test_longest_prefix_match() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("2001:db8::/32", true).unwrap();
        builder.insert("2001:db8:1::/48", false).unwrap();
        builder.insert("2001:db8:1:2::/64", true).unwrap();
        let tree = builder.freeze().unwrap();

        // Should match /64 (most specific)
        let ip1: u128 = Ipv6Addr::new(0x2001, 0xdb8, 1, 2, 0, 0, 0, 1).into();
        assert_eq!(tree.match_ip(ip1), Some(true));

        // Should match /48
        let ip2: u128 = Ipv6Addr::new(0x2001, 0xdb8, 1, 3, 0, 0, 0, 1).into();
        assert_eq!(tree.match_ip(ip2), Some(false));

        // Should match /32
        let ip3: u128 = Ipv6Addr::new(0x2001, 0xdb8, 2, 0, 0, 0, 0, 1).into();
        assert_eq!(tree.match_ip(ip3), Some(true));

        // No match
        let ip4: u128 = Ipv6Addr::new(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1).into();
        assert_eq!(tree.match_ip(ip4), None);
    }

    #[test]
    fn test_host_route() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("2001:db8::1/128", false).unwrap();
        let tree = builder.freeze().unwrap();

        let ip_exact: u128 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1).into();
        let ip_neighbor: u128 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2).into();

        assert_eq!(tree.match_ip(ip_exact), Some(false));
        assert_eq!(tree.match_ip(ip_neighbor), None);
    }

    #[test]
    fn test_default_route() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("::/0", true).unwrap();
        let tree = builder.freeze().unwrap();

        // Should match any IPv6
        let ip1: u128 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1).into();
        let ip2: u128 = Ipv6Addr::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff).into();

        assert_eq!(tree.match_ip(ip1), Some(true));
        assert_eq!(tree.match_ip(ip2), Some(true));
    }
}