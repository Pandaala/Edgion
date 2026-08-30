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
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_named_registration_and_lookup() {
        let idx = RouteDelegationIndex::new();
        idx.update_parent("infra/parent", set(&["billing/child"]), HashSet::new());

        assert_eq!(idx.parents_of("billing/child", "billing"), vec!["infra/parent"]);
        assert!(idx.parents_of("billing/other", "billing").is_empty());
    }

    #[test]
    fn test_wildcard_registration_and_lookup() {
        let idx = RouteDelegationIndex::new();
        idx.update_parent("infra/parent", HashSet::new(), set(&["billing"]));

        // Any route in the wildcard namespace resolves to the parent
        assert_eq!(idx.parents_of("billing/any-route", "billing"), vec!["infra/parent"]);
        // Routes in other namespaces do not
        assert!(idx.parents_of("payments/any-route", "payments").is_empty());
    }

    #[test]
    fn test_named_and_wildcard_union_dedup() {
        let idx = RouteDelegationIndex::new();
        // Same parent declares both a named edge and a wildcard edge covering the child
        idx.update_parent("infra/parent", set(&["billing/child"]), set(&["billing"]));
        idx.update_parent("infra/other-parent", HashSet::new(), set(&["billing"]));

        let parents = idx.parents_of("billing/child", "billing");
        assert_eq!(parents, vec!["infra/other-parent", "infra/parent"]);
    }

    #[test]
    fn test_update_replaces_edges() {
        let idx = RouteDelegationIndex::new();
        idx.update_parent("infra/parent", set(&["billing/child"]), set(&["tenant-a"]));
        assert_eq!(idx.parents_of("billing/child", "billing"), vec!["infra/parent"]);
        assert_eq!(idx.parents_of("tenant-a/x", "tenant-a"), vec!["infra/parent"]);

        // Re-declare with different edges: old ones must disappear
        idx.update_parent("infra/parent", set(&["payments/child2"]), HashSet::new());
        assert!(idx.parents_of("billing/child", "billing").is_empty());
        assert!(idx.parents_of("tenant-a/x", "tenant-a").is_empty());
        assert_eq!(idx.parents_of("payments/child2", "payments"), vec!["infra/parent"]);
    }

    #[test]
    fn test_update_with_empty_sets_clears() {
        let idx = RouteDelegationIndex::new();
        idx.update_parent("infra/parent", set(&["billing/child"]), set(&["billing"]));
        idx.update_parent("infra/parent", HashSet::new(), HashSet::new());
        assert!(idx.parents_of("billing/child", "billing").is_empty());
    }

    #[test]
    fn test_remove_parent() {
        let idx = RouteDelegationIndex::new();
        idx.update_parent("infra/parent", set(&["billing/child"]), set(&["billing"]));
        idx.remove_parent("infra/parent");
        assert!(idx.parents_of("billing/child", "billing").is_empty());
        // Removing again is a no-op
        idx.remove_parent("infra/parent");
    }

    #[test]
    fn test_self_exclusion() {
        let idx = RouteDelegationIndex::new();
        // A parent wildcard-delegates to its own namespace: it must not list
        // itself as its own delegating parent (would requeue itself forever).
        idx.update_parent("infra/parent", HashSet::new(), set(&["infra"]));
        assert!(idx.parents_of("infra/parent", "infra").is_empty());
        // Sibling routes in the namespace still resolve to it
        assert_eq!(idx.parents_of("infra/sibling", "infra"), vec!["infra/parent"]);
    }

    #[test]
    fn test_update_parent_returns_edge_delta() {
        let idx = RouteDelegationIndex::new();
        // First declaration: everything is "changed"
        let d1 = idx.update_parent("infra/parent", set(&["billing/a"]), set(&["tenant-a"]));
        assert_eq!(d1.changed_named, vec!["billing/a"]);
        assert_eq!(d1.changed_namespaces, vec!["tenant-a"]);

        // Same edges again: empty delta
        let d2 = idx.update_parent("infra/parent", set(&["billing/a"]), set(&["tenant-a"]));
        assert!(d2.is_empty());

        // Swap the named child, drop the wildcard: both differences reported
        let d3 = idx.update_parent("infra/parent", set(&["billing/b"]), HashSet::new());
        assert_eq!(d3.changed_named, vec!["billing/a", "billing/b"]);
        assert_eq!(d3.changed_namespaces, vec!["tenant-a"]);
    }

    #[test]
    fn test_clear() {
        let idx = RouteDelegationIndex::new();
        idx.update_parent("infra/parent", set(&["billing/child"]), set(&["billing"]));
        idx.clear();
        assert!(idx.parents_of("billing/child", "billing").is_empty());
    }
}
