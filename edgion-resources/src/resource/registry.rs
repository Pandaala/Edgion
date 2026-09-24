//! Global Resource Type Registry
//!
//! Provides the resource type name list used by `ConfigClient`, built on top of
//! the unified definitions in `resource_defs`.

use super::registry_resource_names;

/// Get the list of all resource type names
pub fn all_resource_type_names() -> Vec<&'static str> {
    registry_resource_names()
}

#[cfg(test)]
#[path = "../../tests/unit/resource/registry_test.rs"]
mod tests;
