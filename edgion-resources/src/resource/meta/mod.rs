//! ResourceMeta trait and implementations
//!
//! This module provides the ResourceMeta trait which defines common operations
//! for all resource types, and implementations using the impl_resource_meta! macro.

mod envelope;
mod impls;
mod traits;

pub use envelope::{extract_resource_metadata, ResourceMetadata};
pub use traits::{extract_sync_version, extract_version, key_name_from_metadata, set_sync_version, ResourceMeta};
