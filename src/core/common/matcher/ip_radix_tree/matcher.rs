//! Unified IP matcher supporting both IPv4 and IPv6
//!
//! This module provides a single interface for matching both IPv4 and IPv6 addresses
//! against CIDR rules, automatically dispatching to the appropriate tree.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

use super::builder_v4::IpV4RadixBuilder;
use super::builder_v6::IpV6RadixBuilder;
use super::error::IpRadixError;
use super::frozen_v4::FrozenIpV4RadixTree;
use super::frozen_v6::FrozenIpV6RadixTree;
use super::types::IpCidr;

/// Maximum number of distinct group labels per matcher.
/// `u16` capacity is 65536; index 0 is reserved as the "no label" sentinel,
/// and `u16::MAX` (65535) is reserved for future use, leaving 1..=65534.
pub const MAX_LABELS: u16 = 65534;

/// A unified IP matcher that supports both IPv4 and IPv6.
///
/// Internally holds separate frozen radix trees for IPv4 and IPv6, plus a
/// sidecar `Vec<Arc<str>>` that maps interned label IDs back to group names.
///
/// # Examples
/// ```
/// use edgion::core::common::matcher::ip_radix_tree::IpRadixMatcher;
/// use std::net::IpAddr;
///
/// let mut builder = IpRadixMatcher::builder();
/// builder.insert("192.168.0.0/16", "office").unwrap();
/// builder.insert("2001:db8::/32", "corp").unwrap();
///
/// let matcher = builder.build().unwrap();
///
/// let ipv4: IpAddr = "192.168.1.50".parse().unwrap();
/// assert_eq!(matcher.matched_group(&ipv4), Some("office"));
///
/// let ipv6: IpAddr = "2001:db8::1".parse().unwrap();
/// assert_eq!(matcher.matched_group(&ipv6), Some("corp"));
/// ```
#[derive(Debug, Clone)]
pub struct IpRadixMatcher {
    ipv4_tree: Option<FrozenIpV4RadixTree>,
    ipv6_tree: Option<FrozenIpV6RadixTree>,
    /// Sidecar: `labels[id]` is the group name for `label_id == id`.
    /// Index 0 is reserved (empty placeholder).
    labels: Vec<Arc<str>>,
}

impl IpRadixMatcher {
    /// Creates a new builder for constructing an IpRadixMatcher
    pub fn builder() -> IpRadixMatcherBuilder {
        IpRadixMatcherBuilder::new()
    }

    /// Zero-allocation membership check. Hot path for RealIpExtractor.
    pub fn contains(&self, ip: &IpAddr) -> bool {
        self.match_label_id(ip).is_some()
    }

    /// Returns the matched group name (longest-prefix). `None` if no rule
    /// covers `ip`. Uses `.get()` — an out-of-bounds `label_id` silently
    /// returns `None` (defensive against builder/labels desync).
    pub fn matched_group(&self, ip: &IpAddr) -> Option<&str> {
        let id = self.match_label_id(ip)?;
        self.labels.get(id as usize).map(|s| s.as_ref())
    }

    /// Like `matched_group` but returns a cloned `Arc`.
    pub fn matched_group_arc(&self, ip: &IpAddr) -> Option<Arc<str>> {
        let id = self.match_label_id(ip)?;
        self.labels.get(id as usize).cloned()
    }

    fn match_label_id(&self, ip: &IpAddr) -> Option<u16> {
        match ip {
            IpAddr::V4(v4) => self.ipv4_tree.as_ref()?.match_ipv4(*v4),
            IpAddr::V6(v6) => {
                if let Some(mapped) = v6.to_ipv4_mapped() {
                    return self.ipv4_tree.as_ref()?.match_ipv4(mapped);
                }
                self.ipv6_tree.as_ref()?.match_ipv6(*v6)
            }
        }
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
/// Collects IPv4 and IPv6 CIDR rules with group labels, then builds separate
/// optimized trees for each IP version plus a shared label sidecar.
#[derive(Debug)]
pub struct IpRadixMatcherBuilder {
    v4_builder: IpV4RadixBuilder,
    v6_builder: IpV6RadixBuilder,
    /// Group name -> label ID. Index 0 reserved.
    label_intern: HashMap<String, u16>,
    /// Parallel to label_intern: labels[id] is the Arc<str> for id.
    labels: Vec<Arc<str>>,
}

impl IpRadixMatcherBuilder {
    /// Creates a new empty builder
    pub fn new() -> Self {
        Self {
            v4_builder: IpV4RadixBuilder::new(),
            v6_builder: IpV6RadixBuilder::new(),
            label_intern: HashMap::new(),
            labels: vec![Arc::from("")], // index 0: reserved sentinel
        }
    }

    /// Insert `(cidr_str, group_name)`. Group name is interned.
    /// Empty `group_name` returns `InvalidCidr` (reserved).
    /// Duplicate CIDR detection at terminal returns `DuplicateCidr` with
    /// real group names resolved from interned IDs.
    /// Hitting `MAX_LABELS + 1` distinct groups returns `TooManyGroups`.
    pub fn insert(&mut self, cidr_str: &str, group_name: &str) -> Result<(), IpRadixError> {
        if group_name.is_empty() {
            return Err(IpRadixError::InvalidCidr {
                input: cidr_str.to_string(),
                reason: "empty group_name".to_string(),
            });
        }

        // Parse first so a bad CIDR doesn't consume an intern slot.
        let cidr = IpCidr::parse(cidr_str)?;
        let id = self.intern(group_name)?;
        let result = match cidr {
            IpCidr::V4 { .. } => self.v4_builder.insert(cidr_str, id),
            IpCidr::V6 { .. } => self.v6_builder.insert(cidr_str, id),
        };
        result.map_err(|e| self.resolve_label_names(e))
    }

    fn intern(&mut self, group_name: &str) -> Result<u16, IpRadixError> {
        if let Some(&id) = self.label_intern.get(group_name) {
            return Ok(id);
        }
        let next_id = self.labels.len() as u32;
        if next_id > MAX_LABELS as u32 {
            return Err(IpRadixError::TooManyGroups { max_labels: MAX_LABELS });
        }
        let id = next_id as u16;
        let arc: Arc<str> = Arc::from(group_name);
        self.labels.push(arc);
        self.label_intern.insert(group_name.to_string(), id);
        Ok(id)
    }

    fn resolve_label_names(&self, err: IpRadixError) -> IpRadixError {
        if let IpRadixError::DuplicateCidr {
            cidr,
            existing_label,
            new_label,
        } = &err
        {
            let resolve = |placeholder: &str| -> String {
                placeholder
                    .strip_prefix('#')
                    .and_then(|s| s.parse::<u16>().ok())
                    .and_then(|id| self.labels.get(id as usize).cloned())
                    .map(|s| s.as_ref().to_string())
                    .unwrap_or_else(|| placeholder.to_string())
            };
            return IpRadixError::DuplicateCidr {
                cidr: cidr.clone(),
                existing_label: resolve(existing_label),
                new_label: resolve(new_label),
            };
        }
        err
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
        let ipv4_tree = if self.v4_builder.is_empty() {
            None
        } else {
            Some(self.v4_builder.freeze()?)
        };
        let ipv6_tree = if self.v6_builder.is_empty() {
            None
        } else {
            Some(self.v6_builder.freeze()?)
        };
        Ok(IpRadixMatcher {
            ipv4_tree,
            ipv6_tree,
            labels: self.labels,
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
    use std::net::Ipv6Addr;

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
        builder.insert("192.168.0.0/16", "g1").unwrap();
        builder.insert("10.0.0.0/8", "g2").unwrap();

        let matcher = builder.build().unwrap();
        assert!(matcher.has_ipv4_rules());
        assert!(!matcher.has_ipv6_rules());

        let ip1: IpAddr = "192.168.1.50".parse().unwrap();
        let ip2: IpAddr = "10.0.0.1".parse().unwrap();
        let ip3: IpAddr = "8.8.8.8".parse().unwrap();

        assert_eq!(matcher.matched_group(&ip1), Some("g1"));
        assert_eq!(matcher.matched_group(&ip2), Some("g2"));
        assert_eq!(matcher.matched_group(&ip3), None);
    }

    #[test]
    fn test_ipv6_only() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("2001:db8::/32", "g1").unwrap();
        builder.insert("fe80::/10", "g2").unwrap();

        let matcher = builder.build().unwrap();
        assert!(!matcher.has_ipv4_rules());
        assert!(matcher.has_ipv6_rules());

        let ip1: IpAddr = "2001:db8::1".parse().unwrap();
        let ip2: IpAddr = "fe80::1".parse().unwrap();
        let ip3: IpAddr = "2001:db9::1".parse().unwrap();

        assert_eq!(matcher.matched_group(&ip1), Some("g1"));
        assert_eq!(matcher.matched_group(&ip2), Some("g2"));
        assert_eq!(matcher.matched_group(&ip3), None);
    }

    #[test]
    fn test_mixed_ipv4_and_ipv6() {
        let mut builder = IpRadixMatcher::builder();

        // IPv4 rules
        builder.insert("192.168.0.0/16", "g1").unwrap();
        builder.insert("10.0.0.0/8", "g2").unwrap();

        // IPv6 rules
        builder.insert("2001:db8::/32", "g1").unwrap();
        builder.insert("fe80::/10", "g2").unwrap();

        let matcher = builder.build().unwrap();
        assert!(matcher.has_ipv4_rules());
        assert!(matcher.has_ipv6_rules());

        // Test IPv4
        let ipv4_1: IpAddr = "192.168.1.50".parse().unwrap();
        let ipv4_2: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(matcher.matched_group(&ipv4_1), Some("g1"));
        assert_eq!(matcher.matched_group(&ipv4_2), Some("g2"));

        // Test IPv6
        let ipv6_1: IpAddr = "2001:db8::1".parse().unwrap();
        let ipv6_2: IpAddr = "fe80::1".parse().unwrap();
        assert_eq!(matcher.matched_group(&ipv6_1), Some("g1"));
        assert_eq!(matcher.matched_group(&ipv6_2), Some("g2"));
    }

    #[test]
    fn test_longest_prefix_match_mixed() {
        let mut builder = IpRadixMatcher::builder();

        // IPv4: broader g1, specific g2
        builder.insert("192.168.0.0/16", "g1").unwrap();
        builder.insert("192.168.1.100/32", "g2").unwrap();

        // IPv6: broader g1, specific g2
        builder.insert("2001:db8::/32", "g1").unwrap();
        builder.insert("2001:db8::1/128", "g2").unwrap();

        let matcher = builder.build().unwrap();

        // IPv4: general IP -> g1, specific IP -> g2
        let ipv4_general: IpAddr = "192.168.1.50".parse().unwrap();
        let ipv4_specific: IpAddr = "192.168.1.100".parse().unwrap();
        assert_eq!(matcher.matched_group(&ipv4_general), Some("g1"));
        assert_eq!(matcher.matched_group(&ipv4_specific), Some("g2"));

        // IPv6: general IP -> g1, specific IP -> g2
        let ipv6_general: IpAddr = "2001:db8::2".parse().unwrap();
        let ipv6_specific: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(matcher.matched_group(&ipv6_general), Some("g1"));
        assert_eq!(matcher.matched_group(&ipv6_specific), Some("g2"));
    }

    #[test]
    fn test_ipv4_mapped_ipv6() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("192.168.1.0/24", "g1").unwrap();
        builder.insert("10.0.0.100/32", "g2").unwrap();

        let matcher = builder.build().unwrap();

        let v6_allow: Ipv6Addr = "::ffff:192.168.1.50".parse().unwrap();
        let mapped_allow = IpAddr::V6(v6_allow);

        let v6_deny: Ipv6Addr = "::ffff:10.0.0.100".parse().unwrap();
        let mapped_deny = IpAddr::V6(v6_deny);

        let v6_no_match: Ipv6Addr = "::ffff:8.8.8.8".parse().unwrap();
        let mapped_no_match = IpAddr::V6(v6_no_match);

        assert_eq!(matcher.matched_group(&mapped_allow), Some("g1"));
        assert_eq!(matcher.matched_group(&mapped_deny), Some("g2"));
        assert_eq!(matcher.matched_group(&mapped_no_match), None);
    }

    #[test]
    fn test_stats() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("192.168.0.0/16", "g1").unwrap();
        builder.insert("2001:db8::/32", "g1").unwrap();

        let matcher = builder.build().unwrap();
        let stats = matcher.stats();

        assert!(stats.ipv4_node_count > 0);
        assert!(stats.ipv6_node_count > 0);
        assert!(stats.total_bytes > 0);
    }

    #[test]
    fn contains_returns_bool_only() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("192.168.0.0/16", "office").unwrap();
        let matcher = builder.build().unwrap();
        let ip1: IpAddr = "192.168.1.50".parse().unwrap();
        let ip2: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(matcher.contains(&ip1));
        assert!(!matcher.contains(&ip2));
    }

    #[test]
    fn duplicate_cidr_across_groups_errors() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("10.0.0.0/24", "office").unwrap();
        let err = builder.insert("10.0.0.0/24", "corp").unwrap_err();
        match err {
            IpRadixError::DuplicateCidr {
                cidr,
                existing_label,
                new_label,
            } => {
                assert_eq!(cidr, "10.0.0.0/24");
                assert_eq!(existing_label, "office");
                assert_eq!(new_label, "corp");
            }
            _ => panic!("wrong variant: {:?}", err),
        }
    }

    #[test]
    fn duplicate_cidr_same_group_is_idempotent() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("10.0.0.0/24", "office").unwrap();
        builder.insert("10.0.0.0/24", "office").unwrap();
    }

    #[test]
    fn matched_group_arc_clones_cheaply() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("10.0.0.0/8", "corp").unwrap();
        let matcher = builder.build().unwrap();
        let ip: IpAddr = "10.5.5.5".parse().unwrap();
        let a = matcher.matched_group_arc(&ip).unwrap();
        let b = matcher.matched_group_arc(&ip).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(&*a, "corp");
    }

    #[test]
    fn ipv4_mapped_ipv6_resolves_to_v4_label() {
        let mut builder = IpRadixMatcher::builder();
        builder.insert("192.168.1.0/24", "office").unwrap();
        let matcher = builder.build().unwrap();
        let mapped: IpAddr = "::ffff:192.168.1.50".parse().unwrap();
        assert_eq!(matcher.matched_group(&mapped), Some("office"));
    }

    #[test]
    fn too_many_groups_returns_error() {
        let mut builder = IpRadixMatcher::builder();
        // Intern MAX_LABELS distinct names (1..=65534). Each succeeds.
        for i in 1..=MAX_LABELS {
            builder.intern(&format!("g{}", i)).unwrap();
        }
        // The 65535th distinct name must fail.
        let err = builder.intern("one_more_group").unwrap_err();
        match err {
            IpRadixError::TooManyGroups { max_labels } => {
                assert_eq!(max_labels, MAX_LABELS);
            }
            other => panic!("expected TooManyGroups, got {:?}", other),
        }
    }
}
