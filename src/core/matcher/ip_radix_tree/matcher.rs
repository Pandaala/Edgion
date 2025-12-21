//! Unified IP matcher supporting both IPv4 and IPv6
//!
//! This module provides a single interface for matching both IPv4 and IPv6 addresses
//! against CIDR rules, automatically dispatching to the appropriate tree.

use super::builder_v4::IpV4RadixBuilder;
use super::builder_v6::IpV6RadixBuilder;
use super::frozen_v4::FrozenIpV4RadixTree;
use super::frozen_v6::FrozenIpV6RadixTree;
use super::error::IpRadixError;
use super::types::IpCidr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A unified IP matcher that supports both IPv4 and IPv6
///
/// This structure contains separate radix trees for IPv4 and IPv6 addresses,
/// automatically dispatching match requests to the appropriate tree.
#[derive(Debug, Clone)]
pub struct IpRadixMatcher {
    /// IPv4 radix tree (optional, created only if IPv4 rules are added)
    ipv4_tree: Option<FrozenIpV4RadixTree>,
    
    /// IPv6 radix tree (optional, created only if IPv6 rules are added)
    ipv6_tree: Option<FrozenIpV6RadixTree>,
}

impl IpRadixMatcher {
    /// Creates a new builder for constructing an IpRadixMatcher
    pub fn builder() -> IpRadixMatcherBuilder {
        IpRadixMatcherBuilder::new()
    }

    /// Matches an IP address against the configured rules
    ///
    /// Automatically detects whether the IP is v4 or v6 and uses the appropriate tree.
    ///
    /// # Arguments
    /// * `ip` - The IP address to match
    ///
    /// # Returns
    /// * `Some(true)` - IP is explicitly allowed
    /// * `Some(false)` - IP is explicitly denied
    /// * `None` - No matching rule found
    ///
    /// # Examples
    /// ```
    /// use edgion::core::routes::http_routes::ip_radix_match::IpRadixMatcher;
    /// use std::net::IpAddr;
    ///
    /// let mut builder = IpRadixMatcher::builder();
    /// builder.insert("192.168.0.0/16", true).unwrap();
    /// builder.insert("2001:db8::/32", false).unwrap();
    ///
    /// let matcher = builder.build().unwrap();
    ///
    /// let ipv4: IpAddr = "192.168.1.50".parse().unwrap();
    /// assert_eq!(matcher.match_ip(&ipv4), Some(true));
    ///
    /// let ipv6: IpAddr = "2001:db8::1".parse().unwrap();
    /// assert_eq!(matcher.match_ip(&ipv6), Some(false));
    /// ```
    pub fn match_ip(&self, ip: &IpAddr) -> Option<bool> {
        match ip {
            IpAddr::V4(ipv4) => self.match_ipv4(ipv4),
            IpAddr::V6(ipv6) => self.match_ipv6(ipv6),
        }
    }

    /// Matches an IPv4 address
    pub fn match_ipv4(&self, ip: &Ipv4Addr) -> Option<bool> {
        self.ipv4_tree.as_ref()?.match_ipv4(*ip)
    }

    /// Matches an IPv6 address
    pub fn match_ipv6(&self, ip: &Ipv6Addr) -> Option<bool> {
        self.ipv6_tree.as_ref()?.match_ipv6(*ip)
    }

    /// Returns whether this matcher has any IPv4 rules
    pub fn has_ipv4_rules(&self) -> bool {
        self.ipv4_tree.is_some()
    }

    /// Returns whether this matcher has any IPv6 rules
    pub fn has_ipv6_rules(&self) -> bool {
        self.ipv6_tree.is_some()
    }

    /// Returns statistics about the matcher
    pub fn stats(&self) -> MatcherStats {
        MatcherStats {
            ipv4_node_count: self.ipv4_tree.as_ref().map_or(0, |t| t.node_count()),
            ipv6_node_count: self.ipv6_tree.as_ref().map_or(0, |t| t.node_count()),
            total_bytes: self.ipv4_tree.as_ref().map_or(0, |t| t.stats().total_bytes)
                + self.ipv6_tree.as_ref().map_or(0, |t| t.stats().total_bytes),
        }
    }
}

/// Statistics about the unified matcher
#[derive(Debug, Clone, Copy)]
pub struct MatcherStats {
    /// Number of IPv4 nodes
    pub ipv4_node_count: usize,
    
    /// Number of IPv6 nodes
    pub ipv6_node_count: usize,
    
    /// Total memory usage in bytes
    pub total_bytes: usize,
}

/// Builder for constructing an IpRadixMatcher
///
/// Collects IPv4 and IPv6 CIDR rules, then builds separate optimized trees
/// for each IP version.
#[derive(Debug)]
pub struct IpRadixMatcherBuilder {
    v4_builder: IpV4RadixBuilder,
    v6_builder: IpV6RadixBuilder,
}

impl IpRadixMatcherBuilder {
    /// Creates a new empty builder
    pub fn new() -> Self {
        Self {
            v4_builder: IpV4RadixBuilder::new(),
            v6_builder: IpV6RadixBuilder::new(),
        }
    }

    /// Inserts a CIDR rule with an allow/deny value
    ///
    /// Automatically detects whether the CIDR is IPv4 or IPv6 and adds it
    /// to the appropriate builder.
    ///
    /// # Arguments
    /// * `cidr_str` - CIDR notation (e.g., "192.168.0.0/16" or "2001:db8::/32")
    /// * `allow` - true for allow, false for deny
    ///
    /// # Examples
    /// ```
    /// use edgion::core::routes::http_routes::ip_radix_match::IpRadixMatcher;
    ///
    /// let mut builder = IpRadixMatcher::builder();
    /// builder.insert("192.168.0.0/16", true).unwrap();
    /// builder.insert("10.0.0.0/8", false).unwrap();
    /// builder.insert("2001:db8::/32", true).unwrap();
    ///
    /// let matcher = builder.build().unwrap();
    /// ```
    pub fn insert(&mut self, cidr_str: &str, allow: bool) -> Result<(), IpRadixError> {
        // Parse CIDR to determine IP version
        let cidr = IpCidr::parse(cidr_str)?;
        
        match cidr {
            IpCidr::V4 { .. } => self.v4_builder.insert(cidr_str, allow),
            IpCidr::V6 { .. } => self.v6_builder.insert(cidr_str, allow),
        }
    }

    /// Returns the number of IPv4 rules added
    pub fn ipv4_rule_count(&self) -> usize {
        self.v4_builder.len()
    }

    /// Returns the number of IPv6 rules added
    pub fn ipv6_rule_count(&self) -> usize {
        self.v6_builder.len()
    }

    /// Builds the IpRadixMatcher
    ///
    /// Freezes both IPv4 and IPv6 builders into optimized trees.
    /// At least one tree (IPv4 or IPv6) must be non-empty.
    ///
    /// # Errors
    /// Returns an error if both trees are empty or if freezing fails
    pub fn build(self) -> Result<IpRadixMatcher, IpRadixError> {
        if self.v4_builder.is_empty() && self.v6_builder.is_empty() {
            return Err(IpRadixError::EmptyTree);
        }

        let ipv4_tree = if !self.v4_builder.is_empty() {
            Some(self.v4_builder.freeze()?)
        } else {
            None
        };

        let ipv6_tree = if !self.v6_builder.is_empty() {
            Some(self.v6_builder.freeze()?)
        } else {
            None
        };

        Ok(IpRadixMatcher {
            ipv4_tree,
            ipv6_tree,
        })
    }
}

impl Default for IpRadixMatcherBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_builder() {
        let builder = IpRadixMatcherBuilder::new();
        assert_eq!(builder.ipv4_rule_count(), 0);
        assert_eq!(builder.ipv6_rule_count(), 0);
        
        let result = builder.build();
        assert!(matches!(result, Err(IpRadixError::EmptyTree)));
    }

    #[test]
    fn test_ipv4_only() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("192.168.0.0/16", true).unwrap();
        builder.insert("10.0.0.0/8", false).unwrap();
        
        let matcher = builder.build().unwrap();
        assert!(matcher.has_ipv4_rules());
        assert!(!matcher.has_ipv6_rules());

        let ip1: IpAddr = "192.168.1.50".parse().unwrap();
        let ip2: IpAddr = "10.0.0.1".parse().unwrap();
        let ip3: IpAddr = "8.8.8.8".parse().unwrap();

        assert_eq!(matcher.match_ip(&ip1), Some(true));
        assert_eq!(matcher.match_ip(&ip2), Some(false));
        assert_eq!(matcher.match_ip(&ip3), None);
    }

    #[test]
    fn test_ipv6_only() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("2001:db8::/32", true).unwrap();
        builder.insert("fe80::/10", false).unwrap();
        
        let matcher = builder.build().unwrap();
        assert!(!matcher.has_ipv4_rules());
        assert!(matcher.has_ipv6_rules());

        let ip1: IpAddr = "2001:db8::1".parse().unwrap();
        let ip2: IpAddr = "fe80::1".parse().unwrap();
        let ip3: IpAddr = "2001:db9::1".parse().unwrap();

        assert_eq!(matcher.match_ip(&ip1), Some(true));
        assert_eq!(matcher.match_ip(&ip2), Some(false));
        assert_eq!(matcher.match_ip(&ip3), None);
    }

    #[test]
    fn test_mixed_ipv4_and_ipv6() {
        let mut builder = IpRadixMatcher::builder();
        
        // IPv4 rules
        builder.insert("192.168.0.0/16", true).unwrap();
        builder.insert("10.0.0.0/8", false).unwrap();
        
        // IPv6 rules
        builder.insert("2001:db8::/32", true).unwrap();
        builder.insert("fe80::/10", false).unwrap();
        
        let matcher = builder.build().unwrap();
        assert!(matcher.has_ipv4_rules());
        assert!(matcher.has_ipv6_rules());

        // Test IPv4
        let ipv4_1: IpAddr = "192.168.1.50".parse().unwrap();
        let ipv4_2: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(matcher.match_ip(&ipv4_1), Some(true));
        assert_eq!(matcher.match_ip(&ipv4_2), Some(false));

        // Test IPv6
        let ipv6_1: IpAddr = "2001:db8::1".parse().unwrap();
        let ipv6_2: IpAddr = "fe80::1".parse().unwrap();
        assert_eq!(matcher.match_ip(&ipv6_1), Some(true));
        assert_eq!(matcher.match_ip(&ipv6_2), Some(false));
    }

    #[test]
    fn test_longest_prefix_match_mixed() {
        let mut builder = IpRadixMatcher::builder();
        
        // IPv4: broader allow, specific deny
        builder.insert("192.168.0.0/16", true).unwrap();
        builder.insert("192.168.1.100/32", false).unwrap();
        
        // IPv6: broader allow, specific deny
        builder.insert("2001:db8::/32", true).unwrap();
        builder.insert("2001:db8::1/128", false).unwrap();
        
        let matcher = builder.build().unwrap();

        // IPv4: general IP allowed, specific IP denied
        let ipv4_general: IpAddr = "192.168.1.50".parse().unwrap();
        let ipv4_specific: IpAddr = "192.168.1.100".parse().unwrap();
        assert_eq!(matcher.match_ip(&ipv4_general), Some(true));
        assert_eq!(matcher.match_ip(&ipv4_specific), Some(false));

        // IPv6: general IP allowed, specific IP denied
        let ipv6_general: IpAddr = "2001:db8::2".parse().unwrap();
        let ipv6_specific: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(matcher.match_ip(&ipv6_general), Some(true));
        assert_eq!(matcher.match_ip(&ipv6_specific), Some(false));
    }

    #[test]
    fn test_stats() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("192.168.0.0/16", true).unwrap();
        builder.insert("2001:db8::/32", true).unwrap();
        
        let matcher = builder.build().unwrap();
        let stats = matcher.stats();
        
        assert!(stats.ipv4_node_count > 0);
        assert!(stats.ipv6_node_count > 0);
        assert!(stats.total_bytes > 0);
    }
}