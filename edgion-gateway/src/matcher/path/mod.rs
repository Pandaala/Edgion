//! A cache-friendly path radix tree for Gateway route and policy matching.
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
//! Algorithm examples and boundary cases live beside the implementation as
//! unit tests because this module is intentionally Gateway-private.

mod builder;
mod error;
mod frozen;

pub(crate) use builder::PathRadixBuilder;
pub(crate) use error::PathRadixError;
pub(crate) use frozen::{PathMatchKind, PathRadixTree};
