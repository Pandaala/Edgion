//! Build-time IPv4 radix tree adapter.

use super::binary::BinaryRadixBuilder;
use super::error::IpRadixError;
use super::frozen_v4::FrozenIpV4RadixTree;
use super::types::IpCidr;

/// A radix tree builder for IPv4 CIDR rules.
#[derive(Debug)]
pub struct IpV4RadixBuilder {
    inner: BinaryRadixBuilder<u32>,
}

impl IpV4RadixBuilder {
    /// Creates a new empty IPv4 radix tree builder.
    pub fn new() -> Self {
        Self {
            inner: BinaryRadixBuilder::new(),
        }
    }

    /// Inserts a CIDR rule with an interned group label ID.
    ///
    /// `label_id == 0` is reserved as the "no label" sentinel and is
    /// rejected in debug builds (the caller must intern before calling).
    ///
    /// Returns `Err(IpRadixError::DuplicateCidr)` if a terminal at the
    /// same normalized CIDR already carries a different label.
    ///
    /// # Examples
    /// ```
    /// use edgion_algo::matcher::radix_ip::builder_v4::IpV4RadixBuilder;
    ///
    /// let mut builder = IpV4RadixBuilder::new();
    /// builder.insert("192.168.0.0/16", 1).unwrap();
    /// builder.insert("192.168.1.100/32", 2).unwrap();
    /// ```
    pub fn insert(&mut self, cidr_str: &str, label_id: u16) -> Result<(), IpRadixError> {
        debug_assert!(label_id != 0, "label_id 0 is reserved");
        match IpCidr::parse(cidr_str)? {
            IpCidr::V4 { addr, prefix_len } => self.inner.insert(addr, prefix_len, label_id, cidr_str),
            IpCidr::V6 { .. } => Err(IpRadixError::InvalidCidr {
                input: cidr_str.to_string(),
                reason: "IPv6 CIDR not supported in IPv4 builder".to_string(),
            }),
        }
    }

    /// Returns whether the builder is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Converts this builder into a cache-friendly frozen tree.
    ///
    /// # Errors
    /// Returns an error if the tree is empty or exceeds size limits during flattening.
    pub fn freeze(self) -> Result<FrozenIpV4RadixTree, IpRadixError> {
        self.inner.freeze().map(FrozenIpV4RadixTree::from_inner)
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
        assert!(builder.is_empty());
    }

    #[test]
    fn test_insert_single_rule() {
        let mut builder = IpV4RadixBuilder::new();
        assert!(builder.insert("192.168.1.0/24", 1).is_ok());
        assert!(!builder.is_empty());
    }

    #[test]
    fn test_insert_multiple_rules() {
        let mut builder = IpV4RadixBuilder::new();
        assert!(builder.insert("10.0.0.0/8", 1).is_ok());
        assert!(builder.insert("192.168.0.0/16", 2).is_ok());
        assert!(builder.insert("172.16.0.0/12", 1).is_ok());
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
        let result = IpV4RadixBuilder::new().freeze();
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
