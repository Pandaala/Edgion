//! Cache-friendly frozen IPv4 radix tree adapter.

use std::net::Ipv4Addr;

use super::binary::FrozenBinaryRadixTree;

/// A cache-friendly, immutable IPv4 radix tree for fast IP matching.
#[repr(transparent)]
#[derive(Debug, Clone)]
pub struct FrozenIpV4RadixTree {
    inner: FrozenBinaryRadixTree<u32>,
}

impl FrozenIpV4RadixTree {
    pub(super) fn from_inner(inner: FrozenBinaryRadixTree<u32>) -> Self {
        Self { inner }
    }

    /// Matches an IPv4 address represented in network byte order.
    ///
    /// Uses longest-prefix matching and returns the most specific label ID.
    ///
    /// # Examples
    /// ```
    /// use edgion_algo::matcher::radix_ip::builder_v4::IpV4RadixBuilder;
    /// use std::net::Ipv4Addr;
    ///
    /// let mut builder = IpV4RadixBuilder::new();
    /// builder.insert("192.168.0.0/16", 1).unwrap();
    /// builder.insert("192.168.1.100/32", 2).unwrap();
    /// let tree = builder.freeze().unwrap();
    ///
    /// let broad: u32 = Ipv4Addr::new(192, 168, 1, 50).into();
    /// let host: u32 = Ipv4Addr::new(192, 168, 1, 100).into();
    /// assert_eq!(tree.match_ip(broad), Some(1));
    /// assert_eq!(tree.match_ip(host), Some(2));
    /// ```
    #[inline]
    pub fn match_ip(&self, ip: u32) -> Option<u16> {
        self.inner.match_key(ip)
    }

    /// Convenience method to match an `Ipv4Addr`.
    #[inline]
    pub fn match_ipv4(&self, ip: Ipv4Addr) -> Option<u16> {
        self.match_ip(u32::from(ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::radix_ip::builder_v4::IpV4RadixBuilder;

    #[test]
    fn test_basic_match() {
        let mut builder = IpV4RadixBuilder::new();
        builder.insert("192.168.1.0/24", 1).unwrap();
        let tree = builder.freeze().unwrap();

        assert_eq!(tree.match_ipv4(Ipv4Addr::new(192, 168, 1, 50)), Some(1));
        assert_eq!(tree.match_ipv4(Ipv4Addr::new(192, 168, 2, 1)), None);
    }

    #[test]
    fn test_longest_prefix_match() {
        let mut builder = IpV4RadixBuilder::new();
        builder.insert("10.0.0.0/8", 1).unwrap();
        builder.insert("10.1.0.0/16", 2).unwrap();
        builder.insert("10.1.1.0/24", 1).unwrap();
        let tree = builder.freeze().unwrap();

        assert_eq!(tree.match_ipv4(Ipv4Addr::new(10, 1, 1, 50)), Some(1));
        assert_eq!(tree.match_ipv4(Ipv4Addr::new(10, 1, 2, 50)), Some(2));
        assert_eq!(tree.match_ipv4(Ipv4Addr::new(10, 2, 1, 50)), Some(1));
        assert_eq!(tree.match_ipv4(Ipv4Addr::new(11, 0, 0, 1)), None);
    }

    #[test]
    fn test_host_route() {
        let mut builder = IpV4RadixBuilder::new();
        builder.insert("192.168.1.100/32", 2).unwrap();
        let tree = builder.freeze().unwrap();

        assert_eq!(tree.match_ipv4(Ipv4Addr::new(192, 168, 1, 100)), Some(2));
        assert_eq!(tree.match_ipv4(Ipv4Addr::new(192, 168, 1, 101)), None);
    }

    #[test]
    fn test_default_route() {
        let mut builder = IpV4RadixBuilder::new();
        builder.insert("0.0.0.0/0", 1).unwrap();
        let tree = builder.freeze().unwrap();

        assert_eq!(tree.match_ipv4(Ipv4Addr::new(1, 2, 3, 4)), Some(1));
        assert_eq!(tree.match_ipv4(Ipv4Addr::new(255, 255, 255, 255)), Some(1));
    }
}
