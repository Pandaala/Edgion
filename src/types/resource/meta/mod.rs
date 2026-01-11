//! ResourceMeta trait and implementations
//!
//! This module provides the ResourceMeta trait which defines common operations
//! for all resource types, and implementations using the impl_resource_meta! macro.

mod impls;
mod traits;

pub use traits::{extract_version, key_name_from_metadata, ResourceMeta};
