//! Global Resource Type Registry
//!
//! This module provides a centralized registry for all resource types in the system.
//! It delegates to the unified resource definitions in `resource_defs.rs`.
//!
//! # Migration Note
//! This module now delegates to `resource_defs` for resource metadata.
//! The `ResourceTypeMetadata` struct and `RESOURCE_TYPES` are kept for backward compatibility.

use std::sync::LazyLock;

use super::{registry_resource_names, ALL_RESOURCE_INFOS};
use crate::core::backends::try_get_global_endpoint_mode;
use crate::core::conf_mgr::conf_center::EndpointMode;

/// Metadata for a resource type
///
/// This struct is kept for backward compatibility.
/// New code should use `ResourceKindInfo` from `resource_defs` module.
#[derive(Debug, Clone)]
pub struct ResourceTypeMetadata {
    /// The name of the resource type (used for display and logging)
    pub name: &'static str,
    /// Description of the resource type (optional)
    pub description: Option<&'static str>,
    /// Whether this resource is a base configuration resource
    pub is_base_conf: bool,
}

impl ResourceTypeMetadata {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            description: None,
            is_base_conf: false,
        }
    }

    pub const fn with_description(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
    }

    pub const fn base_conf(mut self) -> Self {
        self.is_base_conf = true;
        self
    }
}

/// Global registry of all resource types
///
/// This list is generated from `resource_defs.rs` for consistency.
/// Only resources with `in_registry: true` are included (excludes Secret).
pub static RESOURCE_TYPES: LazyLock<Vec<ResourceTypeMetadata>> = LazyLock::new(|| {
    ALL_RESOURCE_INFOS
        .iter()
        .filter(|info| info.in_registry)
        .map(|info| {
            let mut metadata = ResourceTypeMetadata::new(info.cache_field_name);
            if info.is_base_conf {
                metadata = metadata.base_conf();
            }
            // Description is derived from kind_name for simplicity
            metadata = metadata.with_description(info.kind_name);
            metadata
        })
        .collect()
});

/// Get the list of all resource type names
///
/// This function delegates to `resource_defs::registry_resource_names()` and filters
/// based on the global endpoint mode:
/// - `EndpointSlice` mode: excludes "endpoints"
/// - `Endpoint` mode: excludes "endpoint_slices"
/// - Not initialized: returns all resources (for early initialization phases)
pub fn all_resource_type_names() -> Vec<&'static str> {
    let all_names = registry_resource_names();

    // Filter based on endpoint mode if initialized
    match try_get_global_endpoint_mode() {
        Some(EndpointMode::EndpointSlice) => {
            // In EndpointSlice mode, exclude legacy "endpoints"
            all_names.into_iter().filter(|name| *name != "endpoints").collect()
        }
        Some(EndpointMode::Endpoint) => {
            // In Endpoint mode, exclude "endpoint_slices"
            all_names
                .into_iter()
                .filter(|name| *name != "endpoint_slices")
                .collect()
        }
        Some(EndpointMode::Auto) | None => {
            // Auto mode or not initialized - return all (should be resolved before use)
            all_names
        }
    }
}

/// Get the list of base configuration resource names
///
/// This function now delegates to `resource_defs::base_conf_kind_names()`.
pub fn base_conf_resource_names() -> Vec<&'static str> {
    super::base_conf_kind_names()
}

/// Get metadata for a specific resource type by name
pub fn get_resource_metadata(name: &str) -> Option<&ResourceTypeMetadata> {
    RESOURCE_TYPES.iter().find(|r| r.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_registry() {
        // Ensure we have resources registered
        assert!(!RESOURCE_TYPES.is_empty());

        // Check base conf resources
        let base_conf = base_conf_resource_names();
        assert!(base_conf.contains(&"gateway_classes"));
        assert!(base_conf.contains(&"gateways"));
        assert!(base_conf.contains(&"edgion_gateway_configs"));

        // Check non-base conf resources
        let all_names = all_resource_type_names();
        assert!(all_names.contains(&"routes"));
        assert!(all_names.contains(&"services"));
    }

    #[test]
    fn test_metadata_lookup() {
        let metadata = get_resource_metadata("gateway_classes");
        assert!(metadata.is_some());
        assert!(metadata.unwrap().is_base_conf);

        let metadata = get_resource_metadata("routes");
        assert!(metadata.is_some());
        assert!(!metadata.unwrap().is_base_conf);
    }
}
