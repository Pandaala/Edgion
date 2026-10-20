//! Cache-friendly frozen IPv6 radix tree adapter.

use std::net::Ipv6Addr;

use super::binary::FrozenBinaryRadixTree;

/// A cache-friendly, immutable IPv6 radix tree for fast IP matching.
#[repr(transparent)]
#[derive(Debug, Clone)]
pub struct FrozenIpV6RadixTree {
    inner: FrozenBinaryRadixTree<u128>,
}

impl FrozenIpV6RadixTree {
    pub(super) fn from_inner(inner: FrozenBinaryRadixTree<u128>) -> Self {
        Self { inner }
    }

    /// Matches an IPv6 address represented in network byte order.
    ///
    /// Uses longest-prefix matching and returns the most specific label ID.
    ///
    /// # Examples
    /// ```
    /// use edgion_algo::matcher::radix_ip::builder_v6::IpV6RadixBuilder;
    /// use std::net::Ipv6Addr;
    ///
    /// let mut builder = IpV6RadixBuilder::new();
    /// builder.insert("2001:db8::/32", 1).unwrap();
    /// builder.insert("2001:db8::1/128", 2).unwrap();
    /// let tree = builder.freeze().unwrap();
    ///
    /// let broad: u128 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2).into();
    /// let host: u128 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1).into();
    /// assert_eq!(tree.match_ip(broad), Some(1));
    /// assert_eq!(tree.match_ip(host), Some(2));
    /// ```
    #[inline]
    pub fn match_ip(&self, ip: u128) -> Option<u16> {
        self.match_ip_mode::<false>(ip)
    }

    #[inline]
    pub(super) fn match_ip_mode<const FIRST_MATCH: bool>(&self, ip: u128) -> Option<u16> {
        self.inner.match_key::<FIRST_MATCH>(ip)
    }

    /// Convenience method to match an `Ipv6Addr`.
    #[inline]
    pub fn match_ipv6(&self, ip: Ipv6Addr) -> Option<u16> {
        self.match_ip(u128::from(ip))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/matcher/radix_ip/frozen_v6_test.rs"]
mod tests;
