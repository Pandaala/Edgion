//! IP Radix Match - High-performance IP address matching with CIDR support
//!
//! This module provides cache-friendly radix trees for matching IPv4 and IPv6 addresses
//! against CIDR rules. It uses a two-phase approach:
//! 1. Build phase: Use flexible tree structure for easy insertion
//! 2. Freeze phase: Serialize to contiguous memory for cache-friendly lookup
//!
//! # Features
//! - Cache-friendly flat memory layout
//! - Bit-level binary radix tree (2 branches per node)
//! - Longest prefix matching for CIDR rules
//! - Separate trees for IPv4 and IPv6
//! - Simple API focused on allow/deny decisions
//! - Unified matcher that auto-detects IP version
//!
//! # Quick Start
//!
//! For a unified matcher supporting both IPv4 and IPv6:
//!
//! ```
//! use edgion::core::common::matcher::ip_radix_tree::IpRadixMatcher;
//! use std::net::IpAddr;
//!
//! let mut builder = IpRadixMatcher::builder();
//!
//! // Add IPv4 rules
//! builder.insert("192.168.0.0/16", true).unwrap();      // Allow private network
//! builder.insert("192.168.1.100/32", false).unwrap();   // Deny specific IP
//!
//! // Add IPv6 rules
//! builder.insert("2001:db8::/32", true).unwrap();       // Allow
//! builder.insert("fe80::/10", false).unwrap();          // Deny link-local
//!
//! let matcher = builder.build().unwrap();
//!
//! // Match any IP address
//! let ipv4: IpAddr = "192.168.1.50".parse().unwrap();
//! assert_eq!(matcher.match_ip(&ipv4), Some(true));
//!
//! let ipv6: IpAddr = "2001:db8::1".parse().unwrap();
//! assert_eq!(matcher.match_ip(&ipv6), Some(true));
//! ```
//!
//! # IPv4 Only Example
//!
//! For IPv4-only matching:
//!
//! ```
//! use edgion::core::common::matcher::ip_radix_tree::IpV4RadixBuilder;
//! use std::net::Ipv4Addr;
//!
//! let mut builder = IpV4RadixBuilder::new();
//! builder.insert("192.168.0.0/16", true).unwrap();      // Allow
//! builder.insert("192.168.1.100/32", false).unwrap();   // Deny specific IP
//!
//! let tree = builder.freeze().unwrap();
//!
//! let ip1: u32 = Ipv4Addr::new(192, 168, 1, 50).into();
//! assert_eq!(tree.match_ip(ip1), Some(true));
//!
//! let ip2: u32 = Ipv4Addr::new(192, 168, 1, 100).into();
//! assert_eq!(tree.match_ip(ip2), Some(false)); // More specific rule wins
//! ```
//!
//! # Return Values
//!
//! All match functions return `Option<bool>`:
//! - `Some(true)` - IP is explicitly allowed by a matching rule
//! - `Some(false)` - IP is explicitly denied by a matching rule
//! - `None` - No rule matches this IP
//!
//! # Longest Prefix Matching
//!
//! When multiple rules match, the most specific (longest prefix) rule wins:
//!
//! ```
//! use edgion::core::common::matcher::ip_radix_tree::IpV4RadixBuilder;
//! use std::net::Ipv4Addr;
//!
//! let mut builder = IpV4RadixBuilder::new();
//! builder.insert("10.0.0.0/8", true).unwrap();       // Broad allow
//! builder.insert("10.1.0.0/16", false).unwrap();     // Specific deny
//! builder.insert("10.1.1.0/24", true).unwrap();      // Even more specific allow
//!
//! let tree = builder.freeze().unwrap();
//!
//! let ip1: u32 = Ipv4Addr::new(10, 1, 1, 50).into();
//! assert_eq!(tree.match_ip(ip1), Some(true));  // Matches /24 (most specific)
//!
//! let ip2: u32 = Ipv4Addr::new(10, 1, 2, 50).into();
//! assert_eq!(tree.match_ip(ip2), Some(false)); // Matches /16
//!
//! let ip3: u32 = Ipv4Addr::new(10, 2, 1, 50).into();
//! assert_eq!(tree.match_ip(ip3), Some(true));  // Matches /8
//! ```

pub mod builder_v4;
pub mod builder_v6;
pub mod error;
pub mod frozen_v4;
pub mod frozen_v6;
pub mod matcher;
pub mod types;

// Re-export main types for convenience
pub use builder_v4::IpV4RadixBuilder;
pub use builder_v6::IpV6RadixBuilder;
pub use error::IpRadixError;
pub use frozen_v4::FrozenIpV4RadixTree;
pub use frozen_v6::FrozenIpV6RadixTree;
pub use matcher::{IpRadixMatcher, IpRadixMatcherBuilder, MatcherStats};
pub use types::IpCidr;
