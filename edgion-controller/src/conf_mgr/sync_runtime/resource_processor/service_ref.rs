//! Service Reference Manager
//!
//! Tracks which Route resources reference which Services as backends.
//! When a Service changes, dependent routes are requeued so their
//! ResolvedRefs status gets re-evaluated.
//!
//! Uses the same `BidirectionalRefManager` as SecretRefManager.

use super::secret_utils::SecretRefManager;
use super::ResourceRef;

use std::collections::HashSet;

pub type ServiceRefManager = SecretRefManager;

/// Create a new `ServiceRefManager` with the canonical component name.
pub fn new_service_ref_manager() -> ServiceRefManager {
    ServiceRefManager::new()
}

/// Keep mirror-only Service edges distinct from forwarding backend edges.
/// Backend policy ancestor discovery intentionally reads only the unqualified
/// `namespace/name` key because RequestMirror does not support those policies.
pub fn mirror_service_key(namespace: &str, name: &str) -> String {
    mirror_service_dependency_key(&format!("{namespace}/{name}"))
}

fn mirror_service_dependency_key(service_key: &str) -> String {
    format!("request-mirror:{service_key}")
}

/// Return only forwarding backend dependencies for policy ancestor handling.
pub fn forwarding_service_dependencies(manager: &ServiceRefManager, owner_key: &str) -> Vec<String> {
    manager
        .get_dependencies(owner_key)
        .into_iter()
        .filter(|key| !key.starts_with("request-mirror:"))
        .collect()
}

/// Return every resource affected by a Service or EndpointSlice event while
/// deduplicating resources that use the same Service for forwarding and mirror.
pub fn service_event_refs(manager: &ServiceRefManager, service_key: &str) -> Vec<ResourceRef> {
    let mut refs: HashSet<ResourceRef> = manager.get_refs(service_key).into_iter().collect();
    refs.extend(manager.get_refs(&mirror_service_dependency_key(service_key)));
    refs.into_iter().collect()
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/service_ref_test.rs"]
mod tests;
