//! Build-time IPv6 radix tree adapter.

use super::binary::BinaryRadixBuilder;
use super::error::IpRadixError;
use super::frozen_v6::FrozenIpV6RadixTree;
use super::types::IpCidr;

/// A radix tree builder for IPv6 CIDR rules.
#[derive(Debug)]
pub struct IpV6RadixBuilder {
    inner: BinaryRadixBuilder<u128>,
}

impl IpV6RadixBuilder {
    /// Creates a new empty IPv6 radix tree builder.
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
    /// use edgion_algo::matcher::radix_ip::builder_v6::IpV6RadixBuilder;
    ///
    /// let mut builder = IpV6RadixBuilder::new();
    /// builder.insert("2001:db8::/32", 1).unwrap();
    /// builder.insert("fe80::/10", 2).unwrap();
    /// ```
    pub fn insert(&mut self, cidr_str: &str, label_id: u16) -> Result<(), IpRadixError> {
        debug_assert!(label_id != 0, "label_id 0 is reserved");
        match IpCidr::parse(cidr_str)? {
            IpCidr::V6 { addr, prefix_len } => self.inner.insert(addr, prefix_len, label_id, cidr_str),
            IpCidr::V4 { .. } => Err(IpRadixError::InvalidCidr {
                input: cidr_str.to_string(),
                reason: "IPv4 CIDR not supported in IPv6 builder".to_string(),
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
    pub fn freeze(self) -> Result<FrozenIpV6RadixTree, IpRadixError> {
        self.inner.freeze().map(FrozenIpV6RadixTree::from_inner)
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
        assert!(builder.is_empty());
    }

    #[test]
    fn test_insert_single_rule() {
        let mut builder = IpV6RadixBuilder::new();
        assert!(builder.insert("2001:db8::/32", 1).is_ok());
        assert!(!builder.is_empty());
    }

    #[test]
    fn test_insert_multiple_rules() {
        let mut builder = IpV6RadixBuilder::new();
        assert!(builder.insert("2001:db8::/32", 1).is_ok());
        assert!(builder.insert("fe80::/10", 2).is_ok());
        assert!(builder.insert("::1/128", 1).is_ok());
    }

    #[test]
    fn test_reject_ipv4() {
        let mut builder = IpV6RadixBuilder::new();
        let result = builder.insert("192.168.1.0/24", 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_cidr() {
        let mut builder = IpV6RadixBuilder::new();
        assert!(builder.insert("2001:db8::", 1).is_err());
        assert!(builder.insert("invalid/32", 1).is_err());
    }

    #[test]
    fn test_freeze_empty_tree() {
        let result = IpV6RadixBuilder::new().freeze();
        assert!(matches!(result, Err(IpRadixError::EmptyTree)));
    }

    #[test]
    fn duplicate_cidr_detected() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("2001:db8::/32", 1).unwrap();
        let err = builder.insert("2001:db8::/32", 2).unwrap_err();
        assert!(matches!(err, IpRadixError::DuplicateCidr { .. }));
    }

    #[test]
    fn duplicate_cidr_same_label_is_idempotent() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("2001:db8::/32", 1).unwrap();
        assert!(builder.insert("2001:db8::/32", 1).is_ok());
    }
}
