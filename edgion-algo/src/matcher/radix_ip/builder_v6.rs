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
#[path = "../../../tests/unit/matcher/radix_ip/builder_v6_test.rs"]
mod tests;
