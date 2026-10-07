//! ReferenceGrant store with indexed access
//!
//! This module provides global storage for ReferenceGrant resources with
//! efficient lookup by to_namespace for permission checking.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use edgion_resources::resources::refs::{GrantQuery, ResourceScope};
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
    /// Check an owning-field query without interpreting defaults or aliases.
    pub fn check_query(&self, query: GrantQuery<'_>) -> bool {
        let (ResourceScope::Namespaced(source), ResourceScope::Namespaced(target)) =
            (query.source().scope(), query.target().scope())
        else {
            return false;
        };
        if source == target {
            return true;
        }
        self.state
            .read()
            .by_to_namespace
            .get(target)
            .is_some_and(|grants| grants.iter().any(|grant| query.matches(grant)))
    }

    /// Capture permission and its revision from the same authorizer snapshot.
    /// A revoked reference has no revision; an empty set can never authorize
    /// an asynchronous publication captured after revocation.
    pub fn query_authorization_revision(&self, query: GrantQuery<'_>) -> Option<String> {
        let (ResourceScope::Namespaced(source), ResourceScope::Namespaced(target)) =
            (query.source().scope(), query.target().scope())
        else {
            return None;
        };
        if source == target {
            return Some("same-namespace".to_string());
        }
        let state = self.state.read();
        let mut revisions = state
            .by_to_namespace
            .get(target)?
            .iter()
            .filter(|grant| query.matches(grant))
            .map(|grant| {
                format!(
                    "{}:{}",
                    grant.metadata.uid.as_deref().unwrap_or_default(),
                    grant.metadata.resource_version.as_deref().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>();
        if revisions.is_empty() {
            return None;
        }
        revisions.sort();
        Some(revisions.join(","))
    }

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

    /// Stable non-secret identity of all grants currently authorizing a
    /// cross-namespace reference. Callers include it in async-work revisions
    /// so grant replacement invalidates already captured authority.
    #[allow(clippy::too_many_arguments)]
    pub fn authorization_revision(
        &self,
        from_namespace: &str,
        from_group: &str,
        from_kind: &str,
        to_namespace: &str,
        to_group: &str,
        to_kind: &str,
        to_name: Option<&str>,
    ) -> String {
        if from_namespace == to_namespace {
            return "same-namespace".to_string();
        }
        let mut revisions = self
            .get_by_to_namespace(to_namespace)
            .into_iter()
            .filter(|grant| grant.allows_reference(from_namespace, from_group, from_kind, to_group, to_kind, to_name))
            .map(|grant| {
                format!(
                    "{}:{}",
                    grant.metadata.uid.as_deref().unwrap_or_default(),
                    grant.metadata.resource_version.as_deref().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>();
        revisions.sort();
        revisions.join(",")
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
}

impl Default for ReferenceGrantStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/ref_grant/store_test.rs"]
mod tests;
