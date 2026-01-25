//! Cross-Namespace Reference Manager
//!
//! Manages the bidirectional mapping between resources and the namespaces they reference.
//! Used to trigger revalidation when ReferenceGrant changes.
//!
//! Similar to SecretRefManager but indexes by target namespace instead of secret key.

use crate::types::ResourceKind;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

/// Represents a resource that has cross-namespace references
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CrossNsResourceRef {
    pub kind: ResourceKind,
    pub namespace: Option<String>,
    pub name: String,
}

impl CrossNsResourceRef {
    /// Create a new CrossNsResourceRef
    pub fn new(kind: ResourceKind, namespace: Option<String>, name: String) -> Self {
        Self { kind, namespace, name }
    }

    /// Generate a unique key for this resource
    /// Format: "namespace/name" (for namespaced resources)
    pub fn resource_key(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{}/{}", ns, self.name),
            None => self.name.clone(),
        }
    }

    /// Generate a full key including kind
    /// Format: "kind/namespace/name"
    pub fn full_key(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{:?}/{}/{}", self.kind, ns, self.name),
            None => format!("{:?}//{}", self.kind, self.name),
        }
    }

    /// Get the kind as a string (for PROCESSOR_REGISTRY.requeue)
    pub fn kind_str(&self) -> &'static str {
        self.kind.as_str()
    }
}

/// Manages cross-namespace reference mappings
///
/// Provides bidirectional indexing:
/// - Forward: target_namespace → resources that reference it
/// - Reverse: resource → target_namespaces it references
pub struct CrossNamespaceRefManager {
    /// Forward index: target_namespace → resources that reference services in this namespace
    ns_to_resources: RwLock<HashMap<String, HashSet<CrossNsResourceRef>>>,

    /// Reverse index: resource_key → target_namespaces it references
    resource_to_namespaces: RwLock<HashMap<String, HashSet<String>>>,
}

impl CrossNamespaceRefManager {
    /// Create a new CrossNamespaceRefManager
    pub fn new() -> Self {
        Self {
            ns_to_resources: RwLock::new(HashMap::new()),
            resource_to_namespaces: RwLock::new(HashMap::new()),
        }
    }

    /// Add a cross-namespace reference
    ///
    /// Records that `resource_ref` has a reference to a resource in `target_namespace`.
    /// This is idempotent - adding the same reference multiple times is safe.
    pub fn add_cross_ns_ref(&self, target_namespace: String, resource_ref: CrossNsResourceRef) {
        let resource_key = resource_ref.full_key();

        // Add to forward index (namespace → resources)
        {
            let mut ns_map = self.ns_to_resources.write().unwrap();
            ns_map
                .entry(target_namespace.clone())
                .or_default()
                .insert(resource_ref.clone());
        }

        // Add to reverse index (resource → namespaces)
        {
            let mut res_map = self.resource_to_namespaces.write().unwrap();
            res_map
                .entry(resource_key.clone())
                .or_default()
                .insert(target_namespace.clone());
        }

        tracing::debug!(
            component = "cross_ns_ref_manager",
            target_namespace = %target_namespace,
            resource = %resource_key,
            "Added cross-namespace reference"
        );
    }

    /// Get all resources that reference a specific namespace
    ///
    /// Used when ReferenceGrant for `target_namespace` changes to find
    /// which resources need revalidation.
    pub fn get_refs_to_namespace(&self, target_namespace: &str) -> Vec<CrossNsResourceRef> {
        let ns_map = self.ns_to_resources.read().unwrap();
        ns_map
            .get(target_namespace)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get all namespaces that a resource references
    pub fn get_namespaces_for_resource(&self, resource_key: &str) -> Vec<String> {
        let res_map = self.resource_to_namespaces.read().unwrap();
        res_map
            .get(resource_key)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Clear all cross-namespace references for a resource
    ///
    /// Called when:
    /// - Resource is deleted (on_delete)
    /// - Resource is updated (parse, before recording new refs)
    ///
    /// Returns the list of namespaces that were referenced.
    pub fn clear_resource_refs(&self, resource_ref: &CrossNsResourceRef) -> Vec<String> {
        let resource_key = resource_ref.full_key();

        // Get all namespaces this resource references
        let namespaces = {
            let mut res_map = self.resource_to_namespaces.write().unwrap();
            res_map.remove(&resource_key).unwrap_or_default()
        };

        // Remove this resource from all namespace's reference lists
        {
            let mut ns_map = self.ns_to_resources.write().unwrap();
            for ns in &namespaces {
                if let Some(resource_set) = ns_map.get_mut(ns) {
                    resource_set.remove(resource_ref);
                    if resource_set.is_empty() {
                        ns_map.remove(ns);
                    }
                }
            }
        }

        if !namespaces.is_empty() {
            tracing::debug!(
                component = "cross_ns_ref_manager",
                resource = %resource_key,
                namespace_count = namespaces.len(),
                "Cleared cross-namespace references for resource"
            );
        }

        namespaces.into_iter().collect()
    }

    /// Get statistics about the reference manager
    pub fn stats(&self) -> CrossNsRefManagerStats {
        let ns_map = self.ns_to_resources.read().unwrap();
        let res_map = self.resource_to_namespaces.read().unwrap();

        CrossNsRefManagerStats {
            target_namespace_count: ns_map.len(),
            resource_count: res_map.len(),
            total_references: ns_map.values().map(|set| set.len()).sum(),
        }
    }

    /// Get all target namespaces that have resources referencing them
    /// Used for full revalidation after init
    pub fn all_target_namespaces(&self) -> Vec<String> {
        let ns_map = self.ns_to_resources.read().unwrap();
        ns_map.keys().cloned().collect()
    }

    /// Clear all references
    /// Used during relink to reset state
    pub fn clear(&self) {
        {
            let mut ns_map = self.ns_to_resources.write().unwrap();
            ns_map.clear();
        }
        {
            let mut res_map = self.resource_to_namespaces.write().unwrap();
            res_map.clear();
        }
        tracing::info!(
            component = "cross_ns_ref_manager",
            "Cleared all cross-namespace references"
        );
    }
}

impl Default for CrossNamespaceRefManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the cross-namespace reference manager
#[derive(Debug, Clone)]
pub struct CrossNsRefManagerStats {
    /// Number of unique target namespaces being referenced
    pub target_namespace_count: usize,
    /// Number of unique resources with cross-namespace references
    pub resource_count: usize,
    /// Total number of reference relationships
    pub total_references: usize,
}

/// Global cross-namespace reference manager singleton
static GLOBAL_CROSS_NS_REF_MANAGER: OnceLock<Arc<CrossNamespaceRefManager>> = OnceLock::new();

/// Get the global CrossNamespaceRefManager instance
pub fn get_global_cross_ns_ref_manager() -> Arc<CrossNamespaceRefManager> {
    GLOBAL_CROSS_NS_REF_MANAGER
        .get_or_init(|| Arc::new(CrossNamespaceRefManager::new()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cross_ns_resource_ref_keys() {
        let ref1 = CrossNsResourceRef::new(
            ResourceKind::HTTPRoute,
            Some("default".to_string()),
            "my-route".to_string(),
        );
        assert_eq!(ref1.resource_key(), "default/my-route");
        assert_eq!(ref1.full_key(), "HTTPRoute/default/my-route");
        assert_eq!(ref1.kind_str(), "HTTPRoute");
    }

    #[test]
    fn test_add_and_get_refs() {
        let manager = CrossNamespaceRefManager::new();
        let resource =
            CrossNsResourceRef::new(ResourceKind::HTTPRoute, Some("app".to_string()), "my-route".to_string());

        // Route in 'app' namespace references service in 'backend' namespace
        manager.add_cross_ns_ref("backend".to_string(), resource.clone());

        // Check forward lookup
        let refs = manager.get_refs_to_namespace("backend");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0], resource);

        // Check reverse lookup
        let namespaces = manager.get_namespaces_for_resource(&resource.full_key());
        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0], "backend");
    }

    #[test]
    fn test_clear_resource_refs() {
        let manager = CrossNamespaceRefManager::new();
        let resource =
            CrossNsResourceRef::new(ResourceKind::HTTPRoute, Some("app".to_string()), "my-route".to_string());

        // Route references services in multiple namespaces
        manager.add_cross_ns_ref("backend1".to_string(), resource.clone());
        manager.add_cross_ns_ref("backend2".to_string(), resource.clone());

        // Clear all references for this resource
        let cleared = manager.clear_resource_refs(&resource);
        assert_eq!(cleared.len(), 2);
        assert!(cleared.contains(&"backend1".to_string()));
        assert!(cleared.contains(&"backend2".to_string()));

        // Verify cleared
        let refs1 = manager.get_refs_to_namespace("backend1");
        let refs2 = manager.get_refs_to_namespace("backend2");
        assert!(refs1.is_empty());
        assert!(refs2.is_empty());
    }

    #[test]
    fn test_multiple_resources_same_target() {
        let manager = CrossNamespaceRefManager::new();

        let route1 = CrossNsResourceRef::new(ResourceKind::HTTPRoute, Some("app1".to_string()), "route1".to_string());
        let route2 = CrossNsResourceRef::new(ResourceKind::HTTPRoute, Some("app2".to_string()), "route2".to_string());

        // Both routes reference 'backend' namespace
        manager.add_cross_ns_ref("backend".to_string(), route1.clone());
        manager.add_cross_ns_ref("backend".to_string(), route2.clone());

        let refs = manager.get_refs_to_namespace("backend");
        assert_eq!(refs.len(), 2);

        // Clear one resource
        manager.clear_resource_refs(&route1);

        let refs_after = manager.get_refs_to_namespace("backend");
        assert_eq!(refs_after.len(), 1);
        assert_eq!(refs_after[0], route2);
    }

    #[test]
    fn test_idempotent_add() {
        let manager = CrossNamespaceRefManager::new();
        let resource =
            CrossNsResourceRef::new(ResourceKind::HTTPRoute, Some("app".to_string()), "my-route".to_string());

        // Add same reference multiple times
        manager.add_cross_ns_ref("backend".to_string(), resource.clone());
        manager.add_cross_ns_ref("backend".to_string(), resource.clone());
        manager.add_cross_ns_ref("backend".to_string(), resource.clone());

        let refs = manager.get_refs_to_namespace("backend");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn test_stats() {
        let manager = CrossNamespaceRefManager::new();

        let route1 = CrossNsResourceRef::new(ResourceKind::HTTPRoute, Some("app1".to_string()), "route1".to_string());
        let route2 = CrossNsResourceRef::new(ResourceKind::HTTPRoute, Some("app2".to_string()), "route2".to_string());

        manager.add_cross_ns_ref("backend1".to_string(), route1.clone());
        manager.add_cross_ns_ref("backend2".to_string(), route1.clone());
        manager.add_cross_ns_ref("backend1".to_string(), route2.clone());

        let stats = manager.stats();
        assert_eq!(stats.target_namespace_count, 2); // backend1, backend2
        assert_eq!(stats.resource_count, 2); // route1, route2
        assert_eq!(stats.total_references, 3); // 3 relationships
    }
}
