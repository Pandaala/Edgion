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
#[path = "../../../tests/unit/matcher/radix_ip/builder_v4_test.rs"]
mod tests;
