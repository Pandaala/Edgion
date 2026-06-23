//! Build-time IPv4 radix tree implementation
//!
//! This module provides the flexible tree structure used during CIDR rule insertion.

use super::error::IpRadixError;
use super::frozen_v4::FrozenIpV4RadixTree;
use super::types::IpCidr;

/// A binary radix tree node used during the build phase for IPv4
#[derive(Debug, Clone)]
pub(crate) struct IpV4BuildNode {
    pub(crate) left: Option<Box<IpV4BuildNode>>,
    pub(crate) right: Option<Box<IpV4BuildNode>>,
    /// Interned group-label ID at this node. 0 = no rule terminates here.
    pub(crate) label_id: u16,
    pub(crate) prefix_len: u8,
}

impl IpV4BuildNode {
    pub(crate) fn new() -> Self {
        Self {
            left: None,
            right: None,
            label_id: 0,
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

    /// Inserts a CIDR rule with an interned group label ID.
    ///
    /// `label_id == 0` is reserved as the "no label" sentinel and is
    /// rejected (caller must intern before calling).
    ///
    /// Returns `Err(IpRadixError::DuplicateCidr)` if a terminal at the
    /// same normalised CIDR already carries a different label.
    ///
    /// # Examples
    /// ```
    /// use edgion_resources::matcher::ip_radix_tree::builder_v4::IpV4RadixBuilder;
    ///
    /// let mut builder = IpV4RadixBuilder::new();
    /// builder.insert("192.168.0.0/16", 1).unwrap();
    /// builder.insert("192.168.1.100/32", 2).unwrap();
    /// ```
    pub fn insert(&mut self, cidr_str: &str, label_id: u16) -> Result<(), IpRadixError> {
        debug_assert!(label_id != 0, "label_id 0 is reserved");
        let cidr = IpCidr::parse(cidr_str)?;
        match cidr {
            IpCidr::V4 { addr, prefix_len } => {
                Self::insert_helper(&mut self.root, addr, prefix_len, label_id, 0, cidr_str)?;
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
    /// * `label_id` - Interned group label ID (must be non-zero)
    /// * `current_bit` - Current bit position (0-31, from left to right)
    /// * `cidr_str` - Original CIDR string for error reporting
    fn insert_helper(
        node: &mut IpV4BuildNode,
        ip: u32,
        prefix_len: u8,
        label_id: u16,
        current_bit: u8,
        cidr_str: &str,
    ) -> Result<(), IpRadixError> {
        if current_bit == prefix_len {
            if node.label_id != 0 && node.label_id != label_id {
                return Err(IpRadixError::DuplicateCidr {
                    cidr: cidr_str.to_string(),
                    existing_label: format!("#{}", node.label_id),
                    new_label: format!("#{}", label_id),
                });
            }
            node.label_id = label_id;
            node.prefix_len = prefix_len;
            return Ok(());
        }
        let bit = (ip >> (31 - current_bit)) & 1;
        let child = if bit == 0 {
            node.left.get_or_insert_with(|| Box::new(IpV4BuildNode::new()))
        } else {
            node.right.get_or_insert_with(|| Box::new(IpV4BuildNode::new()))
        };
        Self::insert_helper(child, ip, prefix_len, label_id, current_bit + 1, cidr_str)
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
        assert!(builder.insert("192.168.1.0/24", 1).is_ok());
        assert_eq!(builder.len(), 1);
        assert!(!builder.is_empty());
    }

    #[test]
    fn test_insert_multiple_rules() {
        let mut builder = IpV4RadixBuilder::new();
        assert!(builder.insert("10.0.0.0/8", 1).is_ok());
        assert!(builder.insert("192.168.0.0/16", 2).is_ok());
        assert!(builder.insert("172.16.0.0/12", 1).is_ok());
        assert_eq!(builder.len(), 3);
    }

    #[test]
    fn test_reject_ipv6() {
        let mut builder = IpV4RadixBuilder::new();
        let result = builder.insert("2001:db8::/32", 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_cidr() {
        let mut builder = IpV4RadixBuilder::new();
        assert!(builder.insert("192.168.1.0", 1).is_err());
        assert!(builder.insert("invalid/24", 1).is_err());
    }

    #[test]
    fn test_freeze_empty_tree() {
        let builder = IpV4RadixBuilder::new();
        let result = builder.freeze();
        assert!(matches!(result, Err(IpRadixError::EmptyTree)));
    }

    #[test]
    fn duplicate_cidr_detected() {
        let mut builder = IpV4RadixBuilder::new();
        builder.insert("10.0.0.0/24", 1).unwrap();
        let err = builder.insert("10.0.0.0/24", 2).unwrap_err();
        assert!(matches!(err, IpRadixError::DuplicateCidr { .. }));
    }

    #[test]
    fn duplicate_cidr_same_label_is_idempotent() {
        let mut builder = IpV4RadixBuilder::new();
        builder.insert("10.0.0.0/24", 1).unwrap();
        assert!(builder.insert("10.0.0.0/24", 1).is_ok());
    }
}
