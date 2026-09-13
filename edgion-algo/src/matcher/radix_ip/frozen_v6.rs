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
        self.inner.match_key(ip)
    }

    /// Convenience method to match an `Ipv6Addr`.
    #[inline]
    pub fn match_ipv6(&self, ip: Ipv6Addr) -> Option<u16> {
        self.match_ip(u128::from(ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::radix_ip::builder_v6::IpV6RadixBuilder;

    #[test]
    fn test_basic_match() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("2001:db8::/32", 1).unwrap();
        let tree = builder.freeze().unwrap();

        let matching = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
        let non_matching = Ipv6Addr::new(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1);
        assert_eq!(tree.match_ipv6(matching), Some(1));
        assert_eq!(tree.match_ipv6(non_matching), None);
    }

    #[test]
    fn test_longest_prefix_match() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("2001:db8::/32", 1).unwrap();
        builder.insert("2001:db8:1::/48", 2).unwrap();
        builder.insert("2001:db8:1:2::/64", 1).unwrap();
        let tree = builder.freeze().unwrap();

        let prefix_64 = Ipv6Addr::new(0x2001, 0xdb8, 1, 2, 0, 0, 0, 1);
        let prefix_48 = Ipv6Addr::new(0x2001, 0xdb8, 1, 3, 0, 0, 0, 1);
        let prefix_32 = Ipv6Addr::new(0x2001, 0xdb8, 2, 0, 0, 0, 0, 1);
        let no_match = Ipv6Addr::new(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1);
        assert_eq!(tree.match_ipv6(prefix_64), Some(1));
        assert_eq!(tree.match_ipv6(prefix_48), Some(2));
        assert_eq!(tree.match_ipv6(prefix_32), Some(1));
        assert_eq!(tree.match_ipv6(no_match), None);
    }

    #[test]
    fn test_host_route() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("2001:db8::1/128", 2).unwrap();
        let tree = builder.freeze().unwrap();

        let exact = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
        let neighbor = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2);
        assert_eq!(tree.match_ipv6(exact), Some(2));
        assert_eq!(tree.match_ipv6(neighbor), None);
    }

    #[test]
    fn test_default_route() {
        let mut builder = IpV6RadixBuilder::new();
        builder.insert("::/0", 1).unwrap();
        let tree = builder.freeze().unwrap();

        let first = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
        let last = Ipv6Addr::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff);
        assert_eq!(tree.match_ipv6(first), Some(1));
        assert_eq!(tree.match_ipv6(last), Some(1));
    }
}
