//! ReferenceGrant store with indexed access
//!
//! This module provides global storage for ReferenceGrant resources with
//! efficient lookup by to_namespace for permission checking.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::{Arc, RwLock};

use crate::types::resources::ReferenceGrant;

static GLOBAL_REFERENCE_GRANT_STORE: OnceLock<Arc<ReferenceGrantStore>> = OnceLock::new();

pub fn get_global_reference_grant_store() -> Arc<ReferenceGrantStore> {
    GLOBAL_REFERENCE_GRANT_STORE
        .get_or_init(|| Arc::new(ReferenceGrantStore::new()))
        .clone()
}

/// Type alias for the raw grant map (key: namespace/name)
type RawGrantMap = HashMap<String, Arc<ReferenceGrant>>;

/// Type alias for the indexed grant map (key: to_namespace, value: grants in that namespace)
type IndexedGrantMap = HashMap<String, Vec<Arc<ReferenceGrant>>>;

/// Global store for ReferenceGrant resources
///
/// Uses two-layer storage:
/// 1. Raw storage: HashMap<String, Arc<ReferenceGrant>> for basic lookup
/// 2. Indexed storage: HashMap<String, Vec<Arc<ReferenceGrant>>> indexed by to_namespace
///
/// Note: This is control plane code, so we use simple RwLock instead of ArcSwap.
pub struct ReferenceGrantStore {
    /// Raw storage: namespace/name -> ReferenceGrant
    grants: RwLock<RawGrantMap>,

    /// Indexed storage: to_namespace -> Vec<ReferenceGrant>
    /// Used for permission checking
    grants_by_to_namespace: RwLock<IndexedGrantMap>,
}

impl ReferenceGrantStore {
    pub fn new() -> Self {
        Self {
            grants: RwLock::new(HashMap::new()),
            grants_by_to_namespace: RwLock::new(HashMap::new()),
        }
    }

    /// Get a reference grant by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<Arc<ReferenceGrant>> {
        let grants = self.grants.read().unwrap();
        grants.get(key).cloned()
    }

    /// Get a reference grant by namespace and name
    pub fn get_by_ns_name(&self, namespace: &str, name: &str) -> Option<Arc<ReferenceGrant>> {
        let key = format!("{}/{}", namespace, name);
        self.get(&key)
    }

    /// Get all grants that allow references TO a specific namespace
    ///
    /// This is the primary query method for permission checking.
    /// Returns all ReferenceGrants defined in the target namespace.
    pub fn get_by_to_namespace(&self, to_namespace: &str) -> Vec<Arc<ReferenceGrant>> {
        let index = self.grants_by_to_namespace.read().unwrap();
        index.get(to_namespace).cloned().unwrap_or_default()
    }

    /// Check if a reference from (namespace, group, kind) to (namespace, group, kind, name)
    /// is allowed by any ReferenceGrant.
    ///
    /// This is the core permission checking interface.
    ///
    /// # Arguments
    /// * `from_namespace` - Namespace of the source resource (e.g., HTTPRoute's namespace)
    /// * `from_group` - Group of the source resource (e.g., "gateway.networking.k8s.io")
    /// * `from_kind` - Kind of the source resource (e.g., "HTTPRoute")
    /// * `to_namespace` - Namespace of the target resource (e.g., Service's namespace)
    /// * `to_group` - Group of the target resource (e.g., "" for core API)
    /// * `to_kind` - Kind of the target resource (e.g., "Service")
    /// * `to_name` - Optional name of the target resource
    ///
    /// # Returns
    /// `true` if the reference is allowed by at least one ReferenceGrant, `false` otherwise
    #[allow(clippy::too_many_arguments)]
    pub fn check_reference_allowed(
        &self,
        from_namespace: &str,
        from_group: &str,
        from_kind: &str,
        to_namespace: &str,
        to_group: &str,
        to_kind: &str,
        to_name: Option<&str>,
    ) -> bool {
        // Same namespace references are always allowed
        if from_namespace == to_namespace {
            return true;
        }

        // Get all grants in the target namespace
        let grants = self.get_by_to_namespace(to_namespace);

        // Check if any grant allows this reference
        grants
            .iter()
            .any(|grant| grant.allows_reference(from_namespace, from_group, from_kind, to_group, to_kind, to_name))
    }

    /// Add or update a single grant
    pub fn upsert(&self, key: String, grant: Arc<ReferenceGrant>) {
        let to_namespace = grant.namespace().map(|s| s.to_string());

        // Update raw storage
        {
            let mut grants = self.grants.write().unwrap();
            grants.insert(key, grant.clone());
        }

        // Rebuild index for affected namespace
        if let Some(ns) = to_namespace {
            self.rebuild_namespace_index(&ns);
        }
    }

    /// Remove a grant by key
    pub fn remove(&self, key: &str) -> Option<Arc<ReferenceGrant>> {
        // Get the grant first to know which namespace to rebuild
        let grant = {
            let mut grants = self.grants.write().unwrap();
            grants.remove(key)
        };

        // Rebuild index for affected namespace
        if let Some(ref g) = grant {
            if let Some(ns) = g.namespace() {
                self.rebuild_namespace_index(ns);
            }
        }

        grant
    }

    /// Rebuild the index for a specific namespace
    fn rebuild_namespace_index(&self, namespace: &str) {
        let grants = self.grants.read().unwrap();
        let mut index = self.grants_by_to_namespace.write().unwrap();

        let grants_in_ns: Vec<Arc<ReferenceGrant>> = grants
            .values()
            .filter(|g| g.namespace() == Some(namespace))
            .cloned()
            .collect();

        if grants_in_ns.is_empty() {
            index.remove(namespace);
        } else {
            index.insert(namespace.to_string(), grants_in_ns);
        }
    }

    /// Replace all grants and rebuild all indexes
    pub fn replace_all(&self, new_grants: RawGrantMap) {
        // Rebuild index first
        let new_index = Self::build_index(&new_grants);

        // Update both storages
        {
            let mut grants = self.grants.write().unwrap();
            *grants = new_grants;
        }
        {
            let mut index = self.grants_by_to_namespace.write().unwrap();
            *index = new_index;
        }
    }

    /// Update grants incrementally and rebuild affected indexes only
    pub fn update_incremental(
        &self,
        add_or_update: HashMap<String, Arc<ReferenceGrant>>,
        remove: &std::collections::HashSet<String>,
    ) {
        // Identify affected namespaces before updating
        let affected_namespaces = {
            let grants = self.grants.read().unwrap();
            Self::build_affected_namespaces(&add_or_update, remove, &grants)
        };

        // Update raw storage
        {
            let mut grants = self.grants.write().unwrap();

            // Add or update
            for (key, grant) in add_or_update {
                grants.insert(key, grant);
            }

            // Remove
            for key in remove {
                grants.remove(key);
            }
        }

        // Rebuild affected indexes incrementally
        self.rebuild_indexes_incremental(&affected_namespaces);
    }

    /// Build index from raw grants
    fn build_index(grants: &RawGrantMap) -> IndexedGrantMap {
        let mut index: IndexedGrantMap = HashMap::new();

        for grant in grants.values() {
            if let Some(ns) = grant.namespace() {
                index.entry(ns.to_string()).or_default().push(grant.clone());
            }
        }

        index
    }

    /// Identify all namespaces affected by this update
    fn build_affected_namespaces(
        add_or_update: &HashMap<String, Arc<ReferenceGrant>>,
        remove: &std::collections::HashSet<String>,
        current_grants: &RawGrantMap,
    ) -> std::collections::HashSet<String> {
        let mut affected = std::collections::HashSet::new();

        // Extract to_namespace from new/updated grants
        for grant in add_or_update.values() {
            if let Some(ns) = grant.namespace() {
                affected.insert(ns.to_string());
            }
        }

        // Extract to_namespace from removed grants
        for key in remove {
            if let Some(grant) = current_grants.get(key) {
                if let Some(ns) = grant.namespace() {
                    affected.insert(ns.to_string());
                }
            }
        }

        affected
    }

    /// Rebuild indexes for affected namespaces only
    fn rebuild_indexes_incremental(&self, affected_namespaces: &std::collections::HashSet<String>) {
        if affected_namespaces.is_empty() {
            return;
        }

        let grants = self.grants.read().unwrap();
        let mut index = self.grants_by_to_namespace.write().unwrap();

        // Rebuild only affected namespaces
        for ns in affected_namespaces {
            let grants_in_ns: Vec<Arc<ReferenceGrant>> = grants
                .values()
                .filter(|g| g.namespace() == Some(ns.as_str()))
                .cloned()
                .collect();

            if grants_in_ns.is_empty() {
                // Remove empty namespace from index
                index.remove(ns);
            } else {
                // Update namespace index
                index.insert(ns.clone(), grants_in_ns);
            }
        }
    }

    /// Identify all namespaces affected by this update (public for handler use)
    pub fn identify_affected_namespaces(
        &self,
        add: &HashMap<String, ReferenceGrant>,
        update: &HashMap<String, ReferenceGrant>,
        remove: &std::collections::HashSet<String>,
    ) -> std::collections::HashSet<String> {
        let grants = self.grants.read().unwrap();
        let mut affected = std::collections::HashSet::new();

        // Extract from new/updated grants
        for grant in add.values().chain(update.values()) {
            if let Some(ns) = grant.namespace() {
                affected.insert(ns.to_string());
            }
            for from in &grant.spec.from {
                affected.insert(from.namespace.clone());
            }
        }

        // Extract from removed grants
        for key in remove {
            if let Some(grant) = grants.get(key) {
                if let Some(ns) = grant.namespace() {
                    affected.insert(ns.to_string());
                }
                for from in &grant.spec.from {
                    affected.insert(from.namespace.clone());
                }
            }
        }

        affected
    }

    /// Get all grants (for testing/debugging)
    #[cfg(test)]
    pub fn get_all(&self) -> HashMap<String, Arc<ReferenceGrant>> {
        let grants = self.grants.read().unwrap();
        grants.clone()
    }
}

impl Default for ReferenceGrantStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::{ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn create_test_grant(
        namespace: &str,
        name: &str,
        from_namespace: &str,
        from_kind: &str,
        to_kind: &str,
    ) -> ReferenceGrant {
        ReferenceGrant {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantSpec {
                from: vec![ReferenceGrantFrom {
                    group: "gateway.networking.k8s.io".to_string(),
                    kind: from_kind.to_string(),
                    namespace: from_namespace.to_string(),
                }],
                to: vec![ReferenceGrantTo {
                    group: "".to_string(),
                    kind: to_kind.to_string(),
                    name: None,
                }],
            },
        }
    }

    #[test]
    fn test_basic_crud() {
        let store = ReferenceGrantStore::new();
        let grant = create_test_grant("ns-target", "test-grant", "ns-source", "HTTPRoute", "Service");

        let mut grants = HashMap::new();
        grants.insert("ns-target/test-grant".to_string(), Arc::new(grant));

        store.replace_all(grants);

        // Test get
        assert!(store.get("ns-target/test-grant").is_some());
        assert!(store.get("ns-target/nonexistent").is_none());

        // Test get_by_ns_name
        assert!(store.get_by_ns_name("ns-target", "test-grant").is_some());
        assert!(store.get_by_ns_name("ns-target", "nonexistent").is_none());
    }

    #[test]
    fn test_upsert_and_remove() {
        let store = ReferenceGrantStore::new();
        let grant = create_test_grant("ns-target", "test-grant", "ns-source", "HTTPRoute", "Service");

        // Upsert
        store.upsert("ns-target/test-grant".to_string(), Arc::new(grant));
        assert!(store.get("ns-target/test-grant").is_some());
        assert_eq!(store.get_by_to_namespace("ns-target").len(), 1);

        // Remove
        let removed = store.remove("ns-target/test-grant");
        assert!(removed.is_some());
        assert!(store.get("ns-target/test-grant").is_none());
        assert_eq!(store.get_by_to_namespace("ns-target").len(), 0);
    }

    #[test]
    fn test_index_query() {
        let store = ReferenceGrantStore::new();

        let grant1 = create_test_grant("ns1", "grant1", "ns-source", "HTTPRoute", "Service");
        let grant2 = create_test_grant("ns1", "grant2", "ns-source2", "TCPRoute", "Service");
        let grant3 = create_test_grant("ns2", "grant3", "ns-source", "HTTPRoute", "Secret");

        let mut grants = HashMap::new();
        grants.insert("ns1/grant1".to_string(), Arc::new(grant1));
        grants.insert("ns1/grant2".to_string(), Arc::new(grant2));
        grants.insert("ns2/grant3".to_string(), Arc::new(grant3));

        store.replace_all(grants);

        // Query by to_namespace
        let ns1_grants = store.get_by_to_namespace("ns1");
        assert_eq!(ns1_grants.len(), 2);

        let ns2_grants = store.get_by_to_namespace("ns2");
        assert_eq!(ns2_grants.len(), 1);

        let ns_nonexistent = store.get_by_to_namespace("ns-nonexistent");
        assert_eq!(ns_nonexistent.len(), 0);
    }

    #[test]
    fn test_check_reference_allowed_same_namespace() {
        let store = ReferenceGrantStore::new();

        // Same namespace references should always be allowed
        assert!(store.check_reference_allowed(
            "ns1",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "ns1",
            "",
            "Service",
            Some("my-service")
        ));
    }

    #[test]
    fn test_check_reference_allowed_cross_namespace() {
        let store = ReferenceGrantStore::new();

        // Create a grant that allows HTTPRoute from ns-source to access Service in ns-target
        let grant = create_test_grant("ns-target", "test-grant", "ns-source", "HTTPRoute", "Service");
        let mut grants = HashMap::new();
        grants.insert("ns-target/test-grant".to_string(), Arc::new(grant));
        store.replace_all(grants);

        // Should allow: grant exists
        assert!(store.check_reference_allowed(
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "ns-target",
            "",
            "Service",
            Some("my-service")
        ));

        // Should deny: no grant for different source namespace
        assert!(!store.check_reference_allowed(
            "ns-other",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "ns-target",
            "",
            "Service",
            Some("my-service")
        ));
    }
}
