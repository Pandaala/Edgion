//! HTTPRoute Delegation Index
//!
//! Maintains a reverse index from child HTTPRoutes to the parent HTTPRoutes
//! that delegate to them (via `backendRef {kind: HTTPRoute}` rules), so that a
//! child change/delete requeues the affected parents for re-flattening.
//!
//! ## Data Model
//!
//! Named forward index:
//!   child_key (`"{ns}/{name}"`) → Set<parent_key>
//! Wildcard forward index (parent delegates `name: "*"`):
//!   namespace → Set<parent_key>
//! Reverse index (for edge replacement on parent reprocess/delete):
//!   parent_key → (Set<child_key>, Set<namespace>)
//!
//! Edges are declared by the parent (delegation is parent opt-in), so the
//! parent's `parse` replaces its edge set atomically via `update_parent`.
//!
//! ## Concurrency
//!
//! All three maps live under a single `RwLock<Inner>` (same pattern as
//! `GatewayRouteIndex`) so edge replacement and lookup observe a consistent
//! snapshot.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Inner {
    named_forward: HashMap<String, HashSet<String>>,
    wildcard_forward: HashMap<String, HashSet<String>>,
    reverse: HashMap<String, (HashSet<String>, HashSet<String>)>,
}

impl Inner {
    fn remove_parent_inner(&mut self, parent_key: &str) {
        let Some((old_children, old_namespaces)) = self.reverse.remove(parent_key) else {
            return;
        };
        for child in &old_children {
            if let Some(parents) = self.named_forward.get_mut(child) {
                parents.remove(parent_key);
                if parents.is_empty() {
                    self.named_forward.remove(child);
                }
            }
        }
        for ns in &old_namespaces {
            if let Some(parents) = self.wildcard_forward.get_mut(ns) {
                parents.remove(parent_key);
                if parents.is_empty() {
                    self.wildcard_forward.remove(ns);
                }
            }
        }
    }
}

/// Edge changes produced by [`RouteDelegationIndex::update_parent`]:
/// the symmetric difference between the parent's old and new edge sets.
/// The handler requeues these children (and every route in the changed
/// wildcard namespaces) so their delegation status stays fresh.
#[derive(Debug, Default)]
pub struct DelegationEdgeDelta {
    pub changed_named: Vec<String>,
    pub changed_namespaces: Vec<String>,
}

impl DelegationEdgeDelta {
    pub fn is_empty(&self) -> bool {
        self.changed_named.is_empty() && self.changed_namespaces.is_empty()
    }
}

#[derive(Default)]
pub struct RouteDelegationIndex {
    inner: RwLock<Inner>,
}

impl RouteDelegationIndex {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
        }
    }

    /// Replace the delegation edges declared by a parent route.
    ///
    /// Call with empty sets when the parent no longer delegates — this clears
    /// stale edges left by a previous spec revision.
    ///
    /// Returns the symmetric difference against the parent's previous edges so
    /// the caller can requeue affected children for status refresh.
    pub fn update_parent(
        &self,
        parent_key: &str,
        named_children: HashSet<String>,
        wildcard_namespaces: HashSet<String>,
    ) -> DelegationEdgeDelta {
        let mut g = self.inner.write();
        let (old_named, old_ns) = g.reverse.get(parent_key).cloned().unwrap_or_default();

        let mut delta = DelegationEdgeDelta {
            changed_named: old_named.symmetric_difference(&named_children).cloned().collect(),
            changed_namespaces: old_ns.symmetric_difference(&wildcard_namespaces).cloned().collect(),
        };
        delta.changed_named.sort_unstable();
        delta.changed_namespaces.sort_unstable();

        g.remove_parent_inner(parent_key);

        if named_children.is_empty() && wildcard_namespaces.is_empty() {
            return delta;
        }
        for child in &named_children {
            g.named_forward
                .entry(child.clone())
                .or_default()
                .insert(parent_key.to_string());
        }
        for ns in &wildcard_namespaces {
            g.wildcard_forward
                .entry(ns.clone())
                .or_default()
                .insert(parent_key.to_string());
        }
        g.reverse
            .insert(parent_key.to_string(), (named_children, wildcard_namespaces));
        delta
    }

    /// Remove all edges declared by a deleted parent route.
    pub fn remove_parent(&self, parent_key: &str) {
        self.inner.write().remove_parent_inner(parent_key);
    }

    /// All parents that delegate to this route: named edges ∪ wildcard edges
    /// for the route's namespace. Excludes the route itself (a route wildcard-
    /// delegating to its own namespace must not requeue itself forever).
    pub fn parents_of(&self, route_key: &str, route_ns: &str) -> Vec<String> {
        let g = self.inner.read();
        let mut parents: HashSet<&String> = HashSet::new();
        if let Some(named) = g.named_forward.get(route_key) {
            parents.extend(named.iter());
        }
        if let Some(wildcard) = g.wildcard_forward.get(route_ns) {
            parents.extend(wildcard.iter());
        }
        let mut result: Vec<String> = parents
            .into_iter()
            .filter(|p| p.as_str() != route_key)
            .cloned()
            .collect();
        result.sort_unstable();
        result
    }

    /// All routes currently registered as delegating parents.
    /// Used by the post-init revalidation to re-flatten parents whose
    /// children were not yet loaded when the parent was first processed.
    pub fn all_parents(&self) -> Vec<String> {
        let g = self.inner.read();
        let mut keys: Vec<String> = g.reverse.keys().cloned().collect();
        keys.sort_unstable();
        keys
    }

    pub fn clear(&self) {
        let mut g = self.inner.write();
        g.named_forward.clear();
        g.wildcard_forward.clear();
        g.reverse.clear();
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/route_delegation_index_test.rs"]
mod tests;
