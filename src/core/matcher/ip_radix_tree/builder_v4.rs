//! Build-time IPv4 radix tree implementation
//!
//! This module provides the flexible tree structure used during CIDR rule insertion.

use super::error::IpRadixError;
use super::frozen_v4::FrozenIpV4RadixTree;
use super::types::IpCidr;

/// A binary radix tree node used during the build phase for IPv4
///
/// This structure is optimized for easy insertion and modification.
/// Each node represents a decision point in the IP address bit sequence.
#[derive(Debug, Clone)]
pub(crate) struct IpV4BuildNode {
    /// Left child (bit = 0)
    pub(crate) left: Option<Box<IpV4BuildNode>>,

    /// Right child (bit = 1)
    pub(crate) right: Option<Box<IpV4BuildNode>>,

    /// Value at this node: Some(true) = allow, Some(false) = deny, None = no rule
    pub(crate) value: Option<bool>,

    /// Prefix length for this rule (0 if not a terminal node)
    pub(crate) prefix_len: u8,
}

impl IpV4BuildNode {
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

/// A radix tree builder for IPv4 CIDR rules
///
/// This is the mutable builder used to insert CIDR rules. Once all rules are inserted,
/// call `freeze()` to convert it into a cache-friendly `FrozenIpV4RadixTree`.
#[derive(Debug)]
pub struct IpV4RadixBuilder {
    root: IpV4BuildNode,
    rule_count: usize,
}

impl IpV4RadixBuilder {
    /// Creates a new empty IPv4 radix tree builder
    pub fn new() -> Self {
        Self {
            root: IpV4BuildNode::new(),
            rule_count: 0,
        }
    }

    /// Inserts a CIDR rule with an allow/deny value
    ///
    /// # Arguments
    /// * `cidr_str` - The CIDR notation string (e.g., "192.168.1.0/24")
    /// * `allow` - true for allow, false for deny
    ///
    /// # Returns
    /// * `Ok(())` if insertion succeeded
    /// * `Err(IpRadixError)` if insertion fails
    ///
    /// # Examples
    /// ```
    /// use edgion::core::matcher::ip_radix_tree::builder_v4::IpV4RadixBuilder;
    ///
    /// let mut builder = IpV4RadixBuilder::new();
    /// builder.insert("192.168.0.0/16", true).unwrap();
    /// builder.insert("192.168.1.100/32", false).unwrap();
    /// ```
    pub fn insert(&mut self, cidr_str: &str, allow: bool) -> Result<(), IpRadixError> {
        let cidr = IpCidr::parse(cidr_str)?;

        match cidr {
            IpCidr::V4 { addr, prefix_len } => {
                Self::insert_helper(&mut self.root, addr, prefix_len, allow, 0);
                self.rule_count += 1;
                Ok(())
            }
            IpCidr::V6 { .. } => Err(IpRadixError::InvalidCidr {
                input: cidr_str.to_string(),
                reason: "IPv6 CIDR not supported in IPv4 builder".to_string(),
            }),
        }
    }

    /// Helper function for recursive insertion
    ///
    /// # Arguments
    /// * `node` - Current node in the tree
    /// * `ip` - IPv4 address as u32
    /// * `prefix_len` - Prefix length (0-32)
    /// * `allow` - true for allow, false for deny
    /// * `current_bit` - Current bit position (0-31, from left to right)
    fn insert_helper(node: &mut IpV4BuildNode, ip: u32, prefix_len: u8, allow: bool, current_bit: u8) {
        // Base case: reached the prefix length
        if current_bit == prefix_len {
            node.value = Some(allow);
            node.prefix_len = prefix_len;
            return;
        }

        // Get bit at current position (from left/MSB to right/LSB)
        let bit = (ip >> (31 - current_bit)) & 1;

        if bit == 0 {
            // Go left (bit = 0)
            let child = node.left.get_or_insert_with(|| Box::new(IpV4BuildNode::new()));
            Self::insert_helper(child, ip, prefix_len, allow, current_bit + 1);
        } else {
            // Go right (bit = 1)
            let child = node.right.get_or_insert_with(|| Box::new(IpV4BuildNode::new()));
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
    pub fn freeze(self) -> Result<FrozenIpV4RadixTree, IpRadixError> {
        if self.is_empty() {
            return Err(IpRadixError::EmptyTree);
        }
        FrozenIpV4RadixTree::from_builder(self.root)
    }
}

impl Default for IpV4RadixBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_builder() {
        let builder = IpV4RadixBuilder::new();
        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
    }

    #[test]
    fn test_insert_single_rule() {
        let mut builder = IpV4RadixBuilder::new();
        assert!(builder.insert("192.168.1.0/24", true).is_ok());
        assert_eq!(builder.len(), 1);
        assert!(!builder.is_empty());
    }

    #[test]
    fn test_insert_multiple_rules() {
        let mut builder = IpV4RadixBuilder::new();
        assert!(builder.insert("10.0.0.0/8", true).is_ok());
        assert!(builder.insert("192.168.0.0/16", false).is_ok());
        assert!(builder.insert("172.16.0.0/12", true).is_ok());
        assert_eq!(builder.len(), 3);
    }

    #[test]
    fn test_reject_ipv6() {
        let mut builder = IpV4RadixBuilder::new();
        let result = builder.insert("2001:db8::/32", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_cidr() {
        let mut builder = IpV4RadixBuilder::new();
        assert!(builder.insert("192.168.1.0", true).is_err());
        assert!(builder.insert("invalid/24", true).is_err());
    }

    #[test]
    fn test_freeze_empty_tree() {
        let builder = IpV4RadixBuilder::new();
        let result = builder.freeze();
        assert!(matches!(result, Err(IpRadixError::EmptyTree)));
    }
}
