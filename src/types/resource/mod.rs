//! Resource system core module
//!
//! This module contains the core infrastructure for resource type management:
//! - `kind`: ResourceKind enum and conversions
//! - `defs`: Resource definitions using define_resources! macro
//! - `macros`: Macro definitions for resource system
//! - `registry`: Resource type registry and metadata
//! - `meta`: ResourceMeta trait and implementations

// Kind enum must be first (no dependencies)
pub mod kind;

// Macros must be defined before they are used
#[macro_use]
pub mod macros;

// Defs uses macros
pub mod defs;

// Meta traits
pub mod meta;

// Registry uses defs
pub mod registry;

// Re-export core types
pub use kind::ResourceKind;
pub use meta::ResourceMeta;
pub use registry::{
    all_resource_type_names, base_conf_resource_names, get_resource_metadata, ResourceTypeMetadata, RESOURCE_TYPES,
};

// Re-export from defs for backward compatibility
pub use defs::{
    all_resource_kind_names, base_conf_kind_names, get_resource_info, is_kind_cluster_scoped, registry_resource_names,
    resource_kind_from_name, ResourceKindInfo, ALL_RESOURCE_INFOS,
};
