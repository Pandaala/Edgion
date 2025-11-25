//! Radix Route Matcher - A High-Performance Radix Tree Based Route Matching Library
//!
//! This library provides efficient route matching using a Radix Tree (compressed trie) data structure.
//! It's particularly useful for HTTP routers, API gateways, and any application that needs fast
//! prefix-based string matching.
//!
//! # Features
//!
//! - **High Performance**: Based on Redis's `rax` implementation in C
//! - **Multiple Match Modes**: Exact match_engine, longest prefix match_engine, and iterator-based matching
//! - **Safe Rust API**: High-level safe wrapper around C implementation
//! - **C ABI Compatible**: Can be used from C/C++ or other FFI-capable languages
//! - **Unicode Support**: Full UTF-8 support including Chinese characters
//!
//! # Examples
//!
//! ## Basic Usage
//!
//! ```
//! use radix_route_matcher::RadixTree;
//!
//! let mut tree = RadixTree::new().expect("Failed to create tree");
//!
//! // Insert routes
//! tree.insert("/api", 1).unwrap();
//! tree.insert("/api/users", 2).unwrap();
//! tree.insert("/api/posts", 3).unwrap();
//!
//! // Create an iterator for prefix matching operations
//! let iter = tree.create_iter().unwrap();
//!
//! // Exact match_engine
//! assert_eq!(tree.find_exact("/api/users"), Some(2));
//!
//! // Longest prefix match_engine
//! assert_eq!(tree.longest_prefix(&iter, "/api/users/123"), Some(2));
//!
//! // Get all matching prefixes
//! let matches = tree.find_all_prefixes(&iter, "/api/users/123/profile");
//! assert_eq!(matches, vec![2, 1]); // ["/api/users", "/api"]
//! ```
//!
//! ## Iterator-Style Matching
//!
//! ```
//! use radix_route_matcher::RadixTree;
//!
//! let mut tree = RadixTree::new().unwrap();
//! tree.insert("/", 1).unwrap();
//! tree.insert("/api", 2).unwrap();
//! tree.insert("/api/v1", 3).unwrap();
//!
//! let iter = tree.create_iter().unwrap();
//! let path = "/api/v1/users";
//! if tree.search(&iter, path) {
//!     while let Some(idx) = tree.next_prefix(&iter, path) {
//!         println!("Matched route: {}", idx);
//!     }
//! }
//! ```
//!
//! # Performance
//!
//! - Insert: O(k) where k is the key length (~447ns per route)
//! - Query: O(k) where k is the key length (~226ns per query)
//! - Space: Efficient prefix compression, shared prefixes stored once
//!
//! # Module Structure
//!
//! - `ffi`: Low-level FFI bindings to the C rax library
//! - `radix_tree`: High-level safe Rust API (`RadixTree` struct)
//! - `c_api`: C ABI exports for use from other languages

mod c_api;
mod ffi;
mod radix_tree;

#[cfg(test)]
mod tests;

// Re-export the main public API
pub use radix_tree::RadixTree;

// Re-export C API functions for documentation purposes
pub use c_api::{
    radix_tree_destroy, radix_tree_find, radix_tree_insert, radix_tree_new, radix_tree_new_it, radix_tree_remove,
    radix_tree_search, radix_tree_up,
};
