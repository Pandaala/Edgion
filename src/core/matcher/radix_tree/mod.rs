//! A simple, cache-friendly radix tree for route matching
//!
//! This module provides a high-performance prefix tree optimized for CPU cache.
//! It uses a two-phase approach:
//! 1. Build phase: Use flexible tree structure for easy insertion
//! 2. Freeze phase: Serialize to contiguous memory for cache-friendly lookup
//!
//! # Features
//! - Cache-friendly flat memory layout
//! - Zero-copy string storage via string pool
//! - Simple API focused on prefix matching only
//! - No dynamic parameter extraction
//!
//! # Example
//! ```
//! use edgion::core::matcher::radix_tree::{RadixTreeBuilder, RadixTree};
//!
//! let mut builder = RadixTreeBuilder::new();
//! builder.insert("/api/users", 1).unwrap();
//! builder.insert("/api/posts", 2).unwrap();
//! builder.insert("/api/posts/new", 3).unwrap();
//!
//! let tree = builder.freeze().unwrap();
//!
//! assert_eq!(tree.match_route_longest("/api/users"), &[1]);
//! assert_eq!(tree.match_route_longest("/api/posts/new"), &[3]);
//! assert_eq!(tree.match_route_longest("/api/posts/new/edit"), &[3]); // Longest prefix
//! ```

mod builder;
mod error;
mod frozen;

// Re-export with renamed types to hide implementation details
pub use builder::RadixRouter as RadixTreeBuilder;
pub use error::RouterError;
pub use frozen::MatchKind;
pub use frozen::FrozenRadixTree as RadixTree;
