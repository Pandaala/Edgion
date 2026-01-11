//! ResourceMeta trait and implementations
//!
//! This module provides the ResourceMeta trait for Kubernetes resources,
//! combining version information, resource kind, and type metadata.
//!
//! All implementations are consolidated in `impls.rs` using the `impl_resource_meta!` macro.

mod impls;
mod traits;

pub use traits::ResourceMeta;
// Export helper functions for use by impl_resource_meta! macro
pub use traits::{extract_version, key_name_from_metadata};
