//! ReferenceGrant store with indexed access
//!
//! This module provides global storage for ReferenceGrant resources with
//! efficient lookup by to_namespace for permission checking.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use edgion_resources::resources::ReferenceGrant;

/// Type alias for the raw grant map (key: namespace/name)
type RawGrantMap = HashMap<String, Arc<ReferenceGrant>>;

/// Type alias for the indexed grant map (key: to_namespace, value: grants in that namespace)
type IndexedGrantMap = HashMap<String, Vec<Arc<ReferenceGrant>>>;

/// Internal state holding both the raw map and the to_namespace index.
///
/// Both maps live behind a single lock so every mutation atomically updates the
/// index alongside the raw map — no split-lock window where a reader can
/// observe a half-applied change.
struct State {
    raw: RawGrantMap,
    by_to_namespace: IndexedGrantMap,
}

/// Global store for ReferenceGrant resources
///
/// Uses two-layer storage:
/// 1. Raw storage: HashMap<String, Arc<ReferenceGrant>> for basic lookup
/// 2. Indexed storage: HashMap<String, Vec<Arc<ReferenceGrant>>> indexed by to_namespace
///
/// Note: This is control plane code, so we use simple RwLock instead of ArcSwap.
pub struct ReferenceGrantStore {
    state: RwLock<State>,
}

impl ReferenceGrantStore {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(State {
                raw: HashMap::new(),
                by_to_namespace: HashMap::new(),
            }),
        }
    }

    /// Get a reference grant by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<Arc<ReferenceGrant>> {
        self.state.read().raw.get(key).cloned()
    }

    /// Get a reference grant by namespace and name
    #[cfg(test)]
    pub fn get_by_ns_name(&self, namespace: &str, name: &str) -> Option<Arc<ReferenceGrant>> {
        let key = format!("{}/{}", namespace, name);
        self.get(&key)
    }

    /// Get all grants that allow references TO a specific namespace
    ///
    /// This is the primary query method for permission checking.
    /// Returns all ReferenceGrants defined in the target namespace.
    pub fn get_by_to_namespace(&self, to_namespace: &str) -> Vec<Arc<ReferenceGrant>> {
        self.state
            .read()
            .by_to_namespace
            .get(to_namespace)
            .cloned()
            .unwrap_or_default()
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
        // Same-namespace references are always allowed without a grant.
        if from_namespace == to_namespace {
            return true;
        }

        let grants = self.get_by_to_namespace(to_namespace);
        grants
            .iter()
            .any(|grant| grant.allows_reference(from_namespace, from_group, from_kind, to_group, to_kind, to_name))
    }

    /// Add or update a single grant
    pub fn upsert(&self, key: String, grant: Arc<ReferenceGrant>) {
        let to_namespace = grant.namespace().map(|s| s.to_string());

        let mut state = self.state.write();
        state.raw.insert(key, grant);
        if let Some(ns) = to_namespace {
            Self::rebuild_namespace_index_locked(&mut state, &ns);
        }
    }

    /// Remove a grant by key
    pub fn remove(&self, key: &str) -> Option<Arc<ReferenceGrant>> {
        let mut state = self.state.write();
        let grant = state.raw.remove(key)?;
        if let Some(ns) = grant.namespace().map(|s| s.to_string()) {
            Self::rebuild_namespace_index_locked(&mut state, &ns);
        }
        Some(grant)
    }

    /// Rebuild the index for a specific namespace.
    ///
    /// Caller must already hold the state write lock.
    fn rebuild_namespace_index_locked(state: &mut State, namespace: &str) {
        let grants_in_ns: Vec<Arc<ReferenceGrant>> = state
            .raw
            .values()
            .filter(|g| g.namespace() == Some(namespace))
            .cloned()
            .collect();

        if grants_in_ns.is_empty() {
            state.by_to_namespace.remove(namespace);
        } else {
            state.by_to_namespace.insert(namespace.to_string(), grants_in_ns);
        }
    }

    /// Replace all grants and rebuild all indexes
    pub fn replace_all(&self, new_grants: RawGrantMap) {
        let new_index = Self::build_index(&new_grants);
        let mut state = self.state.write();
        state.raw = new_grants;
        state.by_to_namespace = new_index;
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

    /// Get all grants (for testing/debugging)
    #[cfg(test)]
    pub fn get_all(&self) -> HashMap<String, Arc<ReferenceGrant>> {
        self.state.read().raw.clone()
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
    use edgion_resources::resources::{ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
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
