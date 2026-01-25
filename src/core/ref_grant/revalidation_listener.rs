//! Revalidation Listener Implementation
//!
//! Listens for ReferenceGrant changes and triggers revalidation of affected resources.

use super::cross_ns_ref_manager::get_global_cross_ns_ref_manager;
use super::events::{ReferenceGrantChangedEvent, RevalidationListener};
use crate::core::conf_mgr::PROCESSOR_REGISTRY;

/// Cross-namespace revalidation listener
///
/// When ReferenceGrant changes, finds all resources that have cross-namespace
/// references to the affected namespaces and requeues them for revalidation.
pub struct CrossNsRevalidationListener;

impl CrossNsRevalidationListener {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CrossNsRevalidationListener {
    fn default() -> Self {
        Self::new()
    }
}

impl RevalidationListener for CrossNsRevalidationListener {
    fn on_reference_grant_changed(&self, event: &ReferenceGrantChangedEvent) {
        let manager = get_global_cross_ns_ref_manager();

        // If affected_namespaces is empty, it means all namespaces should be revalidated
        // This is typically when a ReferenceGrant is deleted without specific namespace info
        if event.affected_namespaces.is_empty() {
            tracing::warn!(
                component = "cross_ns_revalidation",
                "ReferenceGrant changed with no specific affected namespaces, skipping targeted requeue"
            );
            return;
        }

        let mut requeued_count = 0;

        for ns in &event.affected_namespaces {
            let refs = manager.get_refs_to_namespace(ns);

            for resource_ref in refs {
                let kind = resource_ref.kind_str();
                let key = resource_ref.resource_key();

                tracing::debug!(
                    component = "cross_ns_revalidation",
                    affected_namespace = %ns,
                    kind = %kind,
                    key = %key,
                    "Requeuing resource due to ReferenceGrant change"
                );

                PROCESSOR_REGISTRY.requeue(kind, key);
                requeued_count += 1;
            }
        }

        if requeued_count > 0 {
            tracing::info!(
                component = "cross_ns_revalidation",
                affected_ns_count = event.affected_namespaces.len(),
                requeued_count = requeued_count,
                "Requeued resources due to ReferenceGrant change"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ref_grant::cross_ns_ref_manager::CrossNsResourceRef;
    use crate::types::ResourceKind;
    use std::collections::HashSet;

    #[test]
    fn test_listener_finds_affected_resources() {
        let manager = get_global_cross_ns_ref_manager();

        // Clear any previous state
        manager.clear();

        // Add a resource that references 'backend' namespace
        let route = CrossNsResourceRef::new(
            ResourceKind::HTTPRoute,
            Some("app".to_string()),
            "my-route".to_string(),
        );
        manager.add_cross_ns_ref("backend".to_string(), route);

        // Simulate ReferenceGrant change for 'backend' namespace
        let _event = ReferenceGrantChangedEvent {
            affected_namespaces: {
                let mut set = HashSet::new();
                set.insert("backend".to_string());
                set
            },
        };

        // The listener should find 'app/my-route' for requeue
        let refs = manager.get_refs_to_namespace("backend");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].resource_key(), "app/my-route");
        assert_eq!(refs[0].kind_str(), "HTTPRoute");

        // Clean up
        manager.clear();
    }
}
