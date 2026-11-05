//! Per-key delegation slot table.
//!
//! A slot is the stable hand-off point between a delegating parent rule and the
//! subtree compiled from its child routes. The parent holds an `Arc` to the
//! slot; child rebuilds swap the subtree inside it without touching the parent.

use arc_swap::ArcSwapOption;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::grpc_route::GRPCRoute;
use edgion_resources::resources::http_route::HTTPRoute;
use edgion_resources::resources::route_delegation::{
    classify_parent_refs, DelegationKey, DelegationTargetName, ParentRefShape, RouteKindName,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Atomically swappable subtree holder for one delegation key.
pub struct DelegationSlot<S> {
    subtree: ArcSwapOption<S>,
}

impl<S> DelegationSlot<S> {
    fn new() -> Self {
        Self {
            subtree: ArcSwapOption::empty(),
        }
    }

    /// Current subtree; readers keep it alive across later swaps.
    pub fn load(&self) -> Option<Arc<S>> {
        self.subtree.load_full()
    }

    pub fn store(&self, s: Option<Arc<S>>) {
        self.subtree.store(s);
    }
}

/// Slot table keyed by delegation key. Only touched under the route rebuild lock.
pub struct DelegationSlotTable<S> {
    slots: Mutex<HashMap<DelegationKey, Arc<DelegationSlot<S>>>>,
}

impl<S> Default for DelegationSlotTable<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> DelegationSlotTable<S> {
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }

    pub fn get_or_create(&self, key: &DelegationKey) -> Arc<DelegationSlot<S>> {
        let mut slots = self.slots.lock();
        if let Some(slot) = slots.get(key) {
            return slot.clone();
        }
        let slot = Arc::new(DelegationSlot::new());
        slots.insert(key.clone(), slot.clone());
        slot
    }

    /// Drop touched slots that nobody references and that hold no subtree.
    /// Only the given keys are inspected.
    pub fn retain_referenced(&self, touched: &[DelegationKey]) {
        if touched.is_empty() {
            return;
        }
        let mut slots = self.slots.lock();
        for key in touched {
            let removable = slots
                .get(key)
                .is_some_and(|slot| Arc::strong_count(slot) == 1 && slot.subtree.load().is_none());
            if removable {
                slots.remove(key);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.slots.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Minimal view of a route needed to validate controller-resolved delegation keys.
pub trait RouteParentView {
    fn route_ns(&self) -> &str;
    fn route_name(&self) -> &str;
    fn parent_refs(&self) -> Option<&[ParentReference]>;
    fn route_kind(&self) -> RouteKindName;
}

impl RouteParentView for HTTPRoute {
    fn route_ns(&self) -> &str {
        self.metadata.namespace.as_deref().unwrap_or("default")
    }
    fn route_name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or_default()
    }
    fn parent_refs(&self) -> Option<&[ParentReference]> {
        self.spec.parent_refs.as_deref()
    }
    fn route_kind(&self) -> RouteKindName {
        RouteKindName::HTTPRoute
    }
}

impl RouteParentView for GRPCRoute {
    fn route_ns(&self) -> &str {
        self.metadata.namespace.as_deref().unwrap_or("default")
    }
    fn route_name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or_default()
    }
    fn parent_refs(&self) -> Option<&[ParentReference]> {
        self.spec.parent_refs.as_deref()
    }
    fn route_kind(&self) -> RouteKindName {
        RouteKindName::GRPCRoute
    }
}

/// Keep only keys this child may legitimately publish into: same route kind,
/// parent present in the child's own route-kind parentRefs, target naming this
/// child (by name or `*` in its namespace), and a valid child shape. The Gateway
/// re-checks both directions of the confirmation instead of trusting the key.
pub fn accept_child_keys(child: &impl RouteParentView, keys: &[DelegationKey]) -> Vec<DelegationKey> {
    if keys.is_empty() {
        return Vec::new();
    }
    let kind = child.route_kind();
    let (child_ns, child_name) = (child.route_ns(), child.route_name());
    let ParentRefShape::Child(parents) = classify_parent_refs(child.parent_refs(), child.route_ns(), kind) else {
        return Vec::new();
    };
    keys.iter()
        .filter(|k| {
            k.kind == kind
                && k.target_ns == child_ns
                && match &k.target_name {
                    DelegationTargetName::Named(name) => name == child_name,
                    DelegationTargetName::Wildcard => true,
                }
                && parents
                    .iter()
                    .any(|(ns, name)| *ns == k.parent_ns && *name == k.parent_name)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/delegation/slot_test.rs"]
mod tests;
