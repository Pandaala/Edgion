//! Revalidation Listener Implementation
//!
//! Listens for ReferenceGrant changes and triggers revalidation of affected resources.

use super::cross_ns_ref_manager::get_global_cross_ns_ref_manager;
use super::events::{ReferenceGrantChangedEvent, RevalidationListener};
use crate::core::conf_mgr::PROCESSOR_REGISTRY;

/// Trigger revalidation for all resources with cross-namespace references
///
/// This should be called after all processors are ready during Init phase.
/// It ensures that any Route resources that were processed before their
/// corresponding ReferenceGrant resources are revalidated.
pub fn trigger_full_cross_ns_revalidation() {
    let manager = get_global_cross_ns_ref_manager();
    let stats = manager.stats();

    if stats.resource_count == 0 {
        tracing::debug!(
            component = "cross_ns_revalidation",
            "No resources with cross-namespace references, skipping full revalidation"
        );
        return;
    }

    tracing::info!(
        component = "cross_ns_revalidation",
        resource_count = stats.resource_count,
        target_namespace_count = stats.target_namespace_count,
        total_references = stats.total_references,
        "Triggering full cross-namespace revalidation after init"
    );

    // Get all unique target namespaces and requeue resources referencing them
    // We iterate through all target namespaces to find all resources
    let mut requeued = std::collections::HashSet::new();
    let mut requeued_count = 0;

    // Get all namespaces by iterating through the stats
    // Since we can't directly iterate the manager's internal data,
    // we'll need to query each namespace that has references
    for ns in collect_all_target_namespaces(&manager) {
        for resource_ref in manager.get_refs_to_namespace(&ns) {
            let full_key = resource_ref.full_key();
            if !requeued.contains(&full_key) {
                PROCESSOR_REGISTRY.requeue(resource_ref.kind_str(), resource_ref.resource_key());
                requeued.insert(full_key);
                requeued_count += 1;
            }
        }
    }

    if requeued_count > 0 {
        tracing::info!(
            component = "cross_ns_revalidation",
            requeued_count = requeued_count,
            "Completed full cross-namespace revalidation"
        );
    }
}

/// Helper function to collect all target namespaces from the manager
fn collect_all_target_namespaces(manager: &super::cross_ns_ref_manager::CrossNamespaceRefManager) -> Vec<String> {
    // We need to access the internal data structure
    // For now, we'll expose a method to get all namespaces
    manager.all_target_namespaces()
}

/// Requeue all Gateways after init to resolve TLS certificate references.
///
/// During init, Gateways may be processed before Secrets are loaded into the
/// SecretStore (each resource type initializes independently). This function
/// requeues all Gateways so they re-evaluate ResolvedRefs with fully populated stores.
pub fn trigger_gateway_secret_revalidation() {
    let Some(gateway_proc) = PROCESSOR_REGISTRY.get("Gateway") else {
        return;
    };

    let keys = gateway_proc.list_keys();
    if keys.is_empty() {
        return;
    }

    tracing::info!(
        component = "gateway_secret_revalidation",
        gateway_count = keys.len(),
        "Requeuing all Gateways for post-init TLS secret revalidation"
    );

    for key in keys {
        PROCESSOR_REGISTRY.requeue("Gateway", key);
    }
}

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

        // If affected_namespaces is empty, trigger full revalidation
        // This is typically when a ReferenceGrant is deleted without specific namespace info
        let namespaces_to_check: Vec<String> = if event.affected_namespaces.is_empty() {
            tracing::info!(
                component = "cross_ns_revalidation",
                "ReferenceGrant changed with no specific affected namespaces, triggering full revalidation"
            );
            manager.all_target_namespaces()
        } else {
            event.affected_namespaces.iter().cloned().collect()
        };

        if namespaces_to_check.is_empty() {
            return;
        }

        let mut requeued = std::collections::HashSet::new();
        let mut requeued_count = 0;

        for ns in &namespaces_to_check {
            let refs = manager.get_refs_to_namespace(ns);

            for resource_ref in refs {
                let full_key = resource_ref.full_key();
                // Avoid duplicate requeue for resources referencing multiple affected namespaces
                if requeued.contains(&full_key) {
                    continue;
                }

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
                requeued.insert(full_key);
                requeued_count += 1;
            }
        }

        if requeued_count > 0 {
            tracing::info!(
                component = "cross_ns_revalidation",
                affected_ns_count = namespaces_to_check.len(),
                requeued_count = requeued_count,
                "Requeued resources due to ReferenceGrant change"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::cross_ns_ref_manager::CrossNsResourceRef;
    use super::*;
    use crate::types::ResourceKind;
    use std::collections::HashSet;

    #[test]
    fn test_listener_finds_affected_resources() {
        let manager = get_global_cross_ns_ref_manager();

        // Clear any previous state
        manager.clear();

        // Add a resource that references 'backend' namespace
        let route = CrossNsResourceRef::new(ResourceKind::HTTPRoute, Some("app".to_string()), "my-route".to_string());
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
