//! Build-time IPv6 radix tree implementation
//!
//! This module provides the flexible tree structure used during CIDR rule insertion for IPv6.

use super::error::IpRadixError;
use super::frozen_v6::FrozenIpV6RadixTree;
use super::types::IpCidr;

/// A binary radix tree node used during the build phase for IPv6
///
/// This structure is optimized for easy insertion and modification.
/// Each node represents a decision point in the IPv6 address bit sequence.
#[derive(Debug, Clone)]
pub(crate) struct IpV6BuildNode {
    /// Left child (bit = 0)
    pub(crate) left: Option<Box<IpV6BuildNode>>,

    /// Right child (bit = 1)
    pub(crate) right: Option<Box<IpV6BuildNode>>,

    /// Value at this node: Some(true) = allow, Some(false) = deny, None = no rule
    pub(crate) value: Option<bool>,

    /// Prefix length for this rule (0 if not a terminal node)
    pub(crate) prefix_len: u8,
}

impl IpV6BuildNode {
    /// Create a new empty node
    pub(crate) fn new() -> Self {
        Self {
            left: None,
            right: None,
            value: None,
            prefix_len: 0,
        }
    }
}

/// A radix tree builder for IPv6 CIDR rules
///
/// This is the mutable builder used to insert CIDR rules. Once all rules are inserted,
/// call `freeze()` to convert it into a cache-friendly `FrozenIpV6RadixTree`.
#[derive(Debug)]
pub struct IpV6RadixBuilder {
    root: IpV6BuildNode,
    rule_count: usize,
}

impl IpV6RadixBuilder {
    /// Creates a new empty IPv6 radix tree builder
    pub fn new() -> Self {
        Self {
            root: IpV6BuildNode::new(),
            rule_count: 0,
        }
    }

    /// Inserts a CIDR rule with an allow/deny value
    ///
    /// # Arguments
    /// * `cidr_str` - The CIDR notation string (e.g., "2001:db8::/32")
    /// * `allow` - true for allow, false for deny
    ///
    /// # Returns
    /// * `Ok(())` if insertion succeeded
    /// * `Err(IpRadixError)` if insertion fails
    ///
    /// # Examples
    /// ```
    /// use edgion::core::common::matcher::ip_radix_tree::builder_v6::IpV6RadixBuilder;
    ///
    /// let mut builder = IpV6RadixBuilder::new();
    /// builder.insert("2001:db8::/32", true).unwrap();
    /// builder.insert("fe80::/10", false).unwrap();
    /// ```
    pub fn insert(&mut self, cidr_str: &str, allow: bool) -> Result<(), IpRadixError> {
        let cidr = IpCidr::parse(cidr_str)?;

        match cidr {
            IpCidr::V6 { addr, prefix_len } => {
                Self::insert_helper(&mut self.root, addr, prefix_len, allow, 0);
                self.rule_count += 1;
                Ok(())
            }
            IpCidr::V4 { .. } => Err(IpRadixError::InvalidCidr {
                input: cidr_str.to_string(),
                reason: "IPv4 CIDR not supported in IPv6 builder".to_string(),
            }),
        }
    }

    /// Helper function for recursive insertion
    ///
    /// # Arguments
    /// * `node` - Current node in the tree
    /// * `ip` - IPv6 address as u128
    /// * `prefix_len` - Prefix length (0-128)
    /// * `allow` - true for allow, false for deny
    /// * `current_bit` - Current bit position (0-127, from left to right)
    fn insert_helper(node: &mut IpV6BuildNode, ip: u128, prefix_len: u8, allow: bool, current_bit: u8) {
        // Base case: reached the prefix length
        if current_bit == prefix_len {
            node.value = Some(allow);
            node.prefix_len = prefix_len;
            return;
        }

        // Get bit at current position (from left/MSB to right/LSB)
        let bit = (ip >> (127 - current_bit)) & 1;

        if bit == 0 {
            // Go left (bit = 0)
            let child = node.left.get_or_insert_with(|| Box::new(IpV6BuildNode::new()));
            Self::insert_helper(child, ip, prefix_len, allow, current_bit + 1);
        } else {
            // Go right (bit = 1)
            let child = node.right.get_or_insert_with(|| Box::new(IpV6BuildNode::new()));
            Self::insert_helper(child, ip, prefix_len, allow, current_bit + 1);
        }
    }

    /// Returns the number of rules inserted
    pub fn len(&self) -> usize {
        self.rule_count
    }

    /// Returns whether the builder is empty
    pub fn is_empty(&self) -> bool {
        self.rule_count == 0
    }

    /// Converts this builder into a cache-friendly frozen tree
    ///
    /// After freezing, the tree can no longer be modified, but lookups
    /// will be much faster due to improved memory locality.
    ///
    /// # Errors
    /// Returns an error if the tree is empty or exceeds size limits during flattening
    pub fn freeze(self) -> Result<FrozenIpV6RadixTree, IpRadixError> {
        if self.is_empty() {
            return Err(IpRadixError::EmptyTree);
        }
        FrozenIpV6RadixTree::from_builder(self.root)
    }
}

impl Default for IpV6RadixBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_builder() {
        let builder = IpV6RadixBuilder::new();
        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
    }

    #[test]
    fn test_insert_single_rule() {
        let mut builder = IpV6RadixBuilder::new();
        assert!(builder.insert("2001:db8::/32", true).is_ok());
        assert_eq!(builder.len(), 1);
        assert!(!builder.is_empty());
    }

    #[test]
    fn test_insert_multiple_rules() {
        let mut builder = IpV6RadixBuilder::new();
        assert!(builder.insert("2001:db8::/32", true).is_ok());
        assert!(builder.insert("fe80::/10", false).is_ok());
        assert!(builder.insert("::1/128", true).is_ok());
        assert_eq!(builder.len(), 3);
    }

    #[test]
    fn test_reject_ipv4() {
        let mut builder = IpV6RadixBuilder::new();
        let result = builder.insert("192.168.1.0/24", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_cidr() {
        let mut builder = IpV6RadixBuilder::new();
        assert!(builder.insert("2001:db8::", true).is_err());
        assert!(builder.insert("invalid/32", true).is_err());
    }

    #[test]
    fn test_freeze_empty_tree() {
        let builder = IpV6RadixBuilder::new();
        let result = builder.freeze();
        assert!(matches!(result, Err(IpRadixError::EmptyTree)));
    }
}
