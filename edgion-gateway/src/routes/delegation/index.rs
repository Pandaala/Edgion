//! Gateway-side delegation child index, shared by HTTPRoute and GRPCRoute.
//!
//! Maps each child route key to the delegation keys it was accepted into and
//! its compiled units (`C`, compiled once and `Arc`-shared into every subtree
//! of those keys), plus the reverse key -> child route keys. Only touched under
//! the owning route kind's rebuild lock.

use edgion_resources::resources::route_delegation::DelegationKey;
use std::collections::{BTreeSet, HashMap};

pub(crate) struct ChildEntry<C> {
    pub(crate) keys: Vec<DelegationKey>,
    pub(crate) units: C,
}

pub(crate) struct DelegationChildIndex<C> {
    pub(crate) children: HashMap<String, ChildEntry<C>>,
    pub(crate) key_children: HashMap<DelegationKey, BTreeSet<String>>,
    /// The process Gateway set the cached child units were compiled with
    /// (their leaf metric handles cover exactly these Gateways).
    pub(crate) gateways: Vec<(String, String)>,
}

impl<C> Default for DelegationChildIndex<C> {
    fn default() -> Self {
        Self {
            children: HashMap::new(),
            key_children: HashMap::new(),
            gateways: Vec::new(),
        }
    }
}

impl<C> DelegationChildIndex<C> {
    /// Remove a child; returns the keys it held (whose subtrees must be rebuilt).
    pub(crate) fn unlink(&mut self, child_key: &str) -> Vec<DelegationKey> {
        let Some(entry) = self.children.remove(child_key) else {
            return Vec::new();
        };
        for key in &entry.keys {
            if let Some(children) = self.key_children.get_mut(key) {
                children.remove(child_key);
                if children.is_empty() {
                    self.key_children.remove(key);
                }
            }
        }
        entry.keys
    }

    pub(crate) fn link(&mut self, child_key: &str, keys: Vec<DelegationKey>, units: C) {
        for key in &keys {
            self.key_children
                .entry(key.clone())
                .or_default()
                .insert(child_key.to_string());
        }
        self.children.insert(child_key.to_string(), ChildEntry { keys, units });
    }

    /// Cached units of every current child of `key`, in child-key order.
    pub(crate) fn units_of(&self, key: &DelegationKey) -> impl Iterator<Item = &C> {
        self.key_children
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|child| self.children.get(child).map(|entry| &entry.units))
    }
}
