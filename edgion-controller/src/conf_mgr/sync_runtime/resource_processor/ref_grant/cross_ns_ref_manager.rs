//! Cross-Namespace Reference Manager
//!
//! Type alias over the generic `BidirectionalRefManager` for tracking
//! cross-namespace reference dependencies.
//!
//! When a ReferenceGrant changes, the manager is queried to find all
//! resources that reference the affected namespace so they can be requeued.

use super::super::ref_manager::{BidirectionalRefManager, ResourceRef};

/// Manages cross-namespace reference mappings.
///
/// - Forward: `target_namespace → Set<ResourceRef>` (resources referencing that namespace)
/// - Reverse: `resource_key → Set<target_namespace>` (namespaces a resource references)
pub type CrossNamespaceRefManager = BidirectionalRefManager<ResourceRef>;

/// Create a `CrossNamespaceRefManager` with the canonical component name.
pub fn new_cross_ns_ref_manager() -> CrossNamespaceRefManager {
    CrossNamespaceRefManager::with_component("cross_ns_ref_manager")
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/ref_grant/cross_ns_ref_manager_test.rs"]
mod tests;
