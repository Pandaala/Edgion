//! Cross-Namespace Reference Manager
//!
//! Type alias over the generic `BidirectionalRefManager` for tracking
//! cross-namespace reference dependencies.
//!
//! When a ReferenceGrant changes, the manager is queried to find all
//! resources that reference the affected namespace so they can be requeued.

use super::super::ref_manager::{BidirectionalRefManager, ResourceRef};

// Re-export ResourceRef as CrossNsResourceRef for backward compatibility
// during migration.  New code should use ResourceRef directly.
pub type CrossNsResourceRef = ResourceRef;

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
mod tests {
    use super::*;
    use crate::types::ResourceKind;

    #[test]
    fn test_multiple_resources_same_target() {
        let manager = new_cross_ns_ref_manager();

        let route1 = ResourceRef::new(ResourceKind::HTTPRoute, Some("app1".to_string()), "route1".to_string());
        let route2 = ResourceRef::new(ResourceKind::HTTPRoute, Some("app2".to_string()), "route2".to_string());

        manager.add_ref("backend".to_string(), route1.clone());
        manager.add_ref("backend".to_string(), route2.clone());

        let refs = manager.get_refs("backend");
        assert_eq!(refs.len(), 2);

        manager.clear_resource_refs(&route1);

        let refs_after = manager.get_refs("backend");
        assert_eq!(refs_after.len(), 1);
        assert_eq!(refs_after[0], route2);
    }

    #[test]
    fn test_stats() {
        let manager = new_cross_ns_ref_manager();

        let route1 = ResourceRef::new(ResourceKind::HTTPRoute, Some("app1".to_string()), "route1".to_string());
        let route2 = ResourceRef::new(ResourceKind::HTTPRoute, Some("app2".to_string()), "route2".to_string());

        manager.add_ref("backend1".to_string(), route1.clone());
        manager.add_ref("backend2".to_string(), route1);
        manager.add_ref("backend1".to_string(), route2);

        let stats = manager.stats();
        assert_eq!(stats.source_count, 2); // backend1, backend2
        assert_eq!(stats.value_count, 2); // route1, route2
        assert_eq!(stats.total_references, 3);
    }
}
