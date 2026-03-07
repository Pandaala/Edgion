//! Revalidation Listener Implementation
//!
//! Listens for ReferenceGrant changes and triggers revalidation of affected resources.

use super::cross_ns_ref_manager::get_global_cross_ns_ref_manager;
use super::events::{ReferenceGrantChangedEvent, RevalidationListener};
use crate::core::controller::conf_mgr::PROCESSOR_REGISTRY;

/// Trigger revalidation for all resources with cross-namespace references
///
/// This should be called after all processors are ready during Init phase.
/// It ensures that any Route resources that were processed before their
/// corresponding ReferenceGrant resources are revalidated.
pub fn trigger_full_cross_ns_revalidation() {
    let manager = get_global_cross_ns_ref_manager();
    let stats = manager.stats();

    if stats.value_count == 0 {
        tracing::debug!(
            component = "cross_ns_revalidation",
            "No resources with cross-namespace references, skipping full revalidation"
        );
        return;
    }

    tracing::info!(
        component = "cross_ns_revalidation",
        resource_count = stats.value_count,
        target_namespace_count = stats.source_count,
        total_references = stats.total_references,
        "Triggering full cross-namespace revalidation after init"
    );

    let mut requeued = std::collections::HashSet::new();
    let mut requeued_count = 0;

    for ns in manager.all_source_keys() {
        for resource_ref in manager.get_refs(&ns) {
            let ref_key = resource_ref.key();
            if !requeued.contains(&ref_key) {
                PROCESSOR_REGISTRY.requeue(resource_ref.kind_str(), resource_ref.resource_key());
                requeued.insert(ref_key);
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

        let namespaces_to_check: Vec<String> = if event.affected_namespaces.is_empty() {
            tracing::info!(
                component = "cross_ns_revalidation",
                "ReferenceGrant changed with no specific affected namespaces, triggering full revalidation"
            );
            manager.all_source_keys()
        } else {
            event.affected_namespaces.iter().cloned().collect()
        };

        if namespaces_to_check.is_empty() {
            return;
        }

        let mut requeued = std::collections::HashSet::new();
        let mut requeued_count = 0;

        for ns in &namespaces_to_check {
            let refs = manager.get_refs(ns);

            for resource_ref in refs {
                let ref_key = resource_ref.key();
                if requeued.contains(&ref_key) {
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
                requeued.insert(ref_key);
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
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::ref_manager::ResourceRef;
    use crate::types::ResourceKind;
    use std::collections::HashSet;

    #[test]
    fn test_listener_finds_affected_resources() {
        let manager = get_global_cross_ns_ref_manager();

        manager.clear();

        let route = ResourceRef::new(ResourceKind::HTTPRoute, Some("app".to_string()), "my-route".to_string());
        manager.add_ref("backend".to_string(), route);

        let _event = ReferenceGrantChangedEvent {
            affected_namespaces: {
                let mut set = HashSet::new();
                set.insert("backend".to_string());
                set
            },
        };

        let refs = manager.get_refs("backend");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].resource_key(), "app/my-route");
        assert_eq!(refs[0].kind_str(), "HTTPRoute");

        manager.clear();
    }
}
