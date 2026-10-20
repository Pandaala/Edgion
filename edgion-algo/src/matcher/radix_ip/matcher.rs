//! Unified IP matcher supporting both IPv4 and IPv6
//!
//! This module provides a single interface for matching both IPv4 and IPv6 addresses
//! against CIDR rules, automatically dispatching to the appropriate tree.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
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
/// use edgion_algo::matcher::radix_ip::IpRadixMatcher;
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

    /// Creates a matcher with no rules — always returns `None` for every IP.
    /// Used as the infallible fallback when partial compilation produces an
    /// empty or all-invalid rule set.
    pub fn empty() -> Self {
        Self {
            ipv4_tree: None,
            ipv6_tree: None,
            labels: vec![Arc::from("")], // index 0: reserved sentinel (mirrors builder)
        }
    }

    /// Zero-allocation membership check. Hot path for RealIpExtractor.
    pub fn contains(&self, ip: &IpAddr) -> bool {
        self.match_label_id::<true>(ip).is_some()
    }

    /// Returns the matched group name (longest-prefix). `None` if no rule
    /// covers `ip`. Uses `.get()` — an out-of-bounds `label_id` silently
    /// returns `None` (defensive against builder/labels desync).
    pub fn matched_group(&self, ip: &IpAddr) -> Option<&str> {
        let id = self.match_label_id::<false>(ip)?;
        self.labels.get(id as usize).map(|s| s.as_ref())
    }

    /// Like `matched_group` but returns a cloned `Arc`.
    pub fn matched_group_arc(&self, ip: &IpAddr) -> Option<Arc<str>> {
        let id = self.match_label_id::<false>(ip)?;
        self.labels.get(id as usize).cloned()
    }

    fn match_label_id<const FIRST_MATCH: bool>(&self, ip: &IpAddr) -> Option<u16> {
        match ip {
            IpAddr::V4(v4) => self.ipv4_tree.as_ref()?.match_ip_mode::<FIRST_MATCH>(u32::from(*v4)),
            IpAddr::V6(v6) => {
                if let Some(mapped) = v6.to_ipv4_mapped() {
                    return self.ipv4_tree.as_ref()?.match_ip_mode::<FIRST_MATCH>(u32::from(mapped));
                }
                self.ipv6_tree.as_ref()?.match_ip_mode::<FIRST_MATCH>(u128::from(*v6))
            }
        }
    }
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
            IpCidr::V6 { addr, prefix_len } => {
                // Mapped-only rules must use the same family as mapped queries.
                if let Some(mapped) = Ipv6Addr::from(addr).to_ipv4_mapped().filter(|_| prefix_len >= 96) {
                    self.v4_builder
                        .insert_parsed(u32::from(mapped), prefix_len - 96, id, cidr_str)
                } else {
                    self.v6_builder.insert(cidr_str, id)
                }
            }
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
#[path = "../../../tests/unit/matcher/radix_ip/matcher_test.rs"]
mod tests;
