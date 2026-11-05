//! Participants-only route delegation graph.
//!
//! Tracks only routes that take part in delegation: parents that declare
//! delegating rules, children that declare route-kind parentRefs, and
//! Gateway-attached roots. Ordinary routes never enter the graph, and a
//! wildcard target never scans a namespace: children are found through the
//! declared-children index only.
//!
//! Route keys are `"{namespace}/{name}"`. An edge is confirmed only when both
//! sides agree: the child declares the parent and the parent has a target that
//! matches the child (named equality, or wildcard with the same namespace).
//! Levels are computed over confirmed parents only. A route rejected on its
//! own validation is `blocked`: it keeps its edges but gets no level, so its
//! descendants become `NotAttached`.

use edgion_resources::resources::route_delegation::{
    key_for, DelegationKey, DelegationTarget, DelegationTargetName, RouteKindName, MAX_ROUTE_DELEGATION_LEVELS,
};
use parking_lot::RwLock;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Difference between a parent's old and new target sets.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TargetDelta {
    pub added: Vec<DelegationTarget>,
    pub removed: Vec<DelegationTarget>,
}

/// One valid delegating rule of a parent: its standard filter mask and its
/// statically known delegated scope.
///
/// `scopes` holds the rule's canonical HTTP path prefixes (HTTPRoute) or its
/// delegated service names (GRPCRoute). `None` means the scope cannot be
/// decided statically (for example a prefix with a `:param` segment); such a
/// rule is assumed to reach every child rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatingRule {
    pub mask: u32,
    pub scopes: Option<Vec<String>>,
}

impl DelegatingRule {
    /// A rule with the given mask and an unchecked (unbounded) scope.
    pub fn unscoped(mask: u32) -> Self {
        Self { mask, scopes: None }
    }
}

/// Result of a level recomputation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelChange {
    pub old: Option<u8>,
    pub new: Option<u8>,
}

#[derive(Default)]
struct Inner {
    /// parent key -> (target, the valid delegating rules that write that
    /// target)
    targets_by_parent: HashMap<String, Vec<(DelegationTarget, Vec<DelegatingRule>)>>,
    /// named target key -> parents that delegate to it by name. Lets any route
    /// event (including an ordinary route that never enters the graph) find
    /// the parents whose `BackendNotFound` / `RefNotPermitted` depends on it.
    named_parents: HashMap<String, HashSet<String>>,
    /// parent key -> children declaring it
    declared_by_parent: HashMap<String, HashSet<String>>,
    /// child key -> declaration
    declared_by_child: HashMap<String, Declaration>,
    roots: HashSet<String>,
    /// Participants rejected on their own validation: never given a level.
    blocked: HashSet<String>,
    levels: HashMap<String, u8>,
}

/// A route's declared route-kind parents.
struct Declaration {
    ns: String,
    parents: Vec<String>,
    /// False for an inert route (invalid parentRef shape, or a child with
    /// hostnames): its declaration is visible to parents (so they do not
    /// blame it for `RefNotPermitted`) but it never forms a confirmed edge,
    /// so it gets no keys, no level and is no level dependent.
    confirmable: bool,
}

fn split_key(key: &str) -> (&str, &str) {
    key.split_once('/').unwrap_or(("", key))
}

fn target_matches(target: &DelegationTarget, child_ns: &str, child_name: &str) -> bool {
    target.ns == child_ns
        && match &target.name {
            DelegationTargetName::Named(n) => n == child_name,
            DelegationTargetName::Wildcard => true,
        }
}

impl Inner {
    fn confirmed_keys(&self, kind: RouteKindName, child_key: &str) -> Vec<DelegationKey> {
        let Some(Declaration {
            ns: child_ns,
            parents,
            confirmable: true,
        }) = self.declared_by_child.get(child_key)
        else {
            return Vec::new();
        };
        let (_, child_name) = split_key(child_key);
        let mut out = Vec::new();
        for parent in parents {
            let Some(targets) = self.targets_by_parent.get(parent) else {
                continue;
            };
            let (pns, pname) = split_key(parent);
            for (t, _) in targets.iter().filter(|(t, _)| target_matches(t, child_ns, child_name)) {
                let k = key_for(kind, pns, pname, t);
                if !out.contains(&k) {
                    out.push(k);
                }
            }
        }
        out
    }

    /// True when `parent_key` delegates to `child_key` by name. Only such a
    /// parent's own status (`RefNotPermitted` / `BackendNotFound`) depends on
    /// the child's declared parents; a wildcard target never reports them.
    fn names_child(&self, parent_key: &str, child_key: &str) -> bool {
        self.named_parents
            .get(child_key)
            .is_some_and(|parents| parents.contains(parent_key))
    }

    /// Drop a parent's targets and its named-target index entries.
    fn remove_targets(&mut self, parent_key: &str) -> Vec<(DelegationTarget, Vec<DelegatingRule>)> {
        let old = self.targets_by_parent.remove(parent_key).unwrap_or_default();
        for (target, _) in &old {
            if let DelegationTargetName::Named(name) = &target.name {
                let named_key = format!("{}/{name}", target.ns);
                if let Some(parents) = self.named_parents.get_mut(&named_key) {
                    parents.remove(parent_key);
                    if parents.is_empty() {
                        self.named_parents.remove(&named_key);
                    }
                }
            }
        }
        old
    }
}

pub struct DelegationGraph {
    kind: RouteKindName,
    inner: RwLock<Inner>,
}

impl DelegationGraph {
    pub fn new(kind: RouteKindName) -> Self {
        Self {
            kind,
            inner: RwLock::new(Inner::default()),
        }
    }

    /// Replace a parent's delegation targets (empty removes the parent)
    /// together with the delegating rules writing each target. A target whose
    /// rules changed (mask or scope) is reported in `added`, so its children
    /// are requeued and refresh their conflict and scope status.
    pub fn update_parent(
        &self,
        parent_key: &str,
        targets: Vec<(DelegationTarget, Vec<DelegatingRule>)>,
    ) -> TargetDelta {
        let mut g = self.inner.write();
        let old = g.remove_targets(parent_key);
        let delta = TargetDelta {
            added: targets
                .iter()
                .filter(|entry| !old.contains(entry))
                .map(|(t, _)| t.clone())
                .collect(),
            removed: old
                .iter()
                .filter(|(t, _)| !targets.iter().any(|(new, _)| new == t))
                .map(|(t, _)| t.clone())
                .collect(),
        };
        for (target, _) in &targets {
            if let DelegationTargetName::Named(name) = &target.name {
                g.named_parents
                    .entry(format!("{}/{name}", target.ns))
                    .or_default()
                    .insert(parent_key.to_string());
            }
        }
        if !targets.is_empty() {
            g.targets_by_parent.insert(parent_key.to_string(), targets);
        }
        delta
    }

    /// Parents that delegate to `route_key` by name, sorted. One hash lookup;
    /// valid for any route key, participant or not.
    pub fn parents_naming(&self, route_key: &str) -> Vec<String> {
        let g = self.inner.read();
        let Some(parents) = g.named_parents.get(route_key) else {
            return Vec::new();
        };
        let mut out: Vec<String> = parents.iter().cloned().collect();
        out.sort_unstable();
        out
    }

    /// Every parent that currently has delegation targets, sorted.
    pub fn all_parents(&self) -> Vec<String> {
        let mut out: Vec<String> = self.inner.read().targets_by_parent.keys().cloned().collect();
        out.sort_unstable();
        out
    }

    /// Every delegating rule of `parent_key` whose target covers `child_key`
    /// (named and wildcard targets together); `None` when the parent does
    /// not delegate to it. Bounded by the parent's rule count.
    pub fn edge_rules(&self, parent_key: &str, child_key: &str) -> Option<Vec<DelegatingRule>> {
        let g = self.inner.read();
        let (child_ns, child_name) = split_key(child_key);
        let mut matched = g
            .targets_by_parent
            .get(parent_key)?
            .iter()
            .filter(|(t, _)| target_matches(t, child_ns, child_name))
            .peekable();
        matched.peek()?;
        Some(matched.flat_map(|(_, rules)| rules.iter().cloned()).collect())
    }

    /// Replace a child's declared parents. Returns the old and new declared
    /// parents that delegate to this child by name: only their status reads
    /// the declaration. A parent reached through a wildcard target (or one
    /// that does not target the child at all) is never returned, so a route
    /// in another namespace cannot force an unrelated parent to republish.
    /// A non-`confirmable` declaration (inert route) is recorded for
    /// `declared_parents` only and never forms a confirmed edge.
    pub fn update_child(
        &self,
        child_key: &str,
        child_ns: &str,
        declared_parents: Vec<String>,
        confirmable: bool,
    ) -> Vec<String> {
        let mut g = self.inner.write();
        let mut affected: BTreeSet<String> = BTreeSet::new();
        if let Some(old) = g.declared_by_child.remove(child_key) {
            for p in old.parents {
                if let Some(children) = g.declared_by_parent.get_mut(&p) {
                    children.remove(child_key);
                    if children.is_empty() {
                        g.declared_by_parent.remove(&p);
                    }
                }
                if g.names_child(&p, child_key) {
                    affected.insert(p);
                }
            }
        }
        for p in &declared_parents {
            g.declared_by_parent
                .entry(p.clone())
                .or_default()
                .insert(child_key.to_string());
            if g.names_child(p, child_key) {
                affected.insert(p.clone());
            }
        }
        if !declared_parents.is_empty() {
            g.declared_by_child.insert(
                child_key.to_string(),
                Declaration {
                    ns: child_ns.to_string(),
                    parents: declared_parents,
                    confirmable,
                },
            );
        }
        affected.into_iter().collect()
    }

    /// Parent keys currently declared by `child_key` (empty when none).
    pub fn declared_parents(&self, child_key: &str) -> Vec<String> {
        self.inner
            .read()
            .declared_by_child
            .get(child_key)
            .map(|declaration| declaration.parents.clone())
            .unwrap_or_default()
    }

    /// Mark a participant route as Gateway-attached (level 1) or not.
    pub fn set_root(&self, route_key: &str, attached: bool) {
        let mut g = self.inner.write();
        if attached {
            g.roots.insert(route_key.to_string());
        } else {
            g.roots.remove(route_key);
        }
    }

    /// Mark a participant route as rejected on its own validation (no level)
    /// or not. Returns true when the flag changed.
    pub fn set_blocked(&self, route_key: &str, blocked: bool) -> bool {
        let mut g = self.inner.write();
        if blocked {
            g.blocked.insert(route_key.to_string())
        } else {
            g.blocked.remove(route_key)
        }
    }

    /// True when the route is rejected on its own validation.
    pub fn is_blocked(&self, route_key: &str) -> bool {
        self.inner.read().blocked.contains(route_key)
    }

    /// Confirmable children that declare `parent_key` and match `target`.
    pub fn children_of_target(&self, parent_key: &str, target: &DelegationTarget) -> Vec<String> {
        let g = self.inner.read();
        let Some(children) = g.declared_by_parent.get(parent_key) else {
            return Vec::new();
        };
        let mut out: Vec<String> = children
            .iter()
            .filter(|c| {
                g.declared_by_child
                    .get(*c)
                    .is_some_and(|d| d.confirmable && target_matches(target, &d.ns, split_key(c).1))
            })
            .cloned()
            .collect();
        out.sort_unstable();
        out
    }

    /// Delegation keys of every confirmed edge into `child_key`.
    pub fn confirmed_keys(&self, child_key: &str) -> Vec<DelegationKey> {
        self.inner.read().confirmed_keys(self.kind, child_key)
    }

    /// Recompute a route's level from its root flag and confirmed parents.
    pub fn recompute_level(&self, route_key: &str) -> LevelChange {
        let mut g = self.inner.write();
        let old = g.levels.get(route_key).copied();
        let new = if g.blocked.contains(route_key) {
            None
        } else if g.roots.contains(route_key) {
            Some(1u8)
        } else {
            let min_parent = g
                .confirmed_keys(self.kind, route_key)
                .iter()
                .filter_map(|k| g.levels.get(&format!("{}/{}", k.parent_ns, k.parent_name)).copied())
                .min();
            min_parent
                .map(|l| l as usize + 1)
                .filter(|l| *l <= MAX_ROUTE_DELEGATION_LEVELS)
                .map(|l| l as u8)
        };
        match new {
            Some(l) => {
                g.levels.insert(route_key.to_string(), l);
            }
            None => {
                g.levels.remove(route_key);
            }
        }
        LevelChange { old, new }
    }

    pub fn level(&self, route_key: &str) -> Option<u8> {
        self.inner.read().levels.get(route_key).copied()
    }

    /// All confirmed children of a parent (for level requeue).
    pub fn children_of(&self, parent_key: &str) -> Vec<String> {
        let g = self.inner.read();
        let (Some(targets), Some(children)) = (
            g.targets_by_parent.get(parent_key),
            g.declared_by_parent.get(parent_key),
        ) else {
            return Vec::new();
        };
        let mut out: Vec<String> = children
            .iter()
            .filter(|c| {
                g.declared_by_child.get(*c).is_some_and(|d| {
                    let name = split_key(c).1;
                    d.confirmable && targets.iter().any(|(t, _)| target_matches(t, &d.ns, name))
                })
            })
            .cloned()
            .collect();
        out.sort_unstable();
        out
    }

    pub fn is_participant(&self, key: &str) -> bool {
        let g = self.inner.read();
        g.targets_by_parent.contains_key(key) || g.declared_by_child.contains_key(key) || g.roots.contains(key)
    }

    /// Remove a route entirely. Returns keys to requeue: its declared
    /// children (as a parent) and those declared parents that delegate to it
    /// by name (as a child; see [`Self::update_child`]).
    pub fn remove(&self, key: &str) -> Vec<String> {
        let mut g = self.inner.write();
        let mut requeue: BTreeSet<String> = BTreeSet::new();
        if let Some(children) = g.declared_by_parent.get(key) {
            requeue.extend(children.iter().cloned());
        }
        g.remove_targets(key);
        if let Some(declaration) = g.declared_by_child.remove(key) {
            for p in declaration.parents {
                if let Some(children) = g.declared_by_parent.get_mut(&p) {
                    children.remove(key);
                    if children.is_empty() {
                        g.declared_by_parent.remove(&p);
                    }
                }
                if g.names_child(&p, key) {
                    requeue.insert(p);
                }
            }
        }
        g.roots.remove(key);
        g.blocked.remove(key);
        g.levels.remove(key);
        requeue.remove(key);
        requeue.into_iter().collect()
    }

    /// Route delete: the keys to requeue are the parents naming the route
    /// plus, for a participant, everything [`Self::remove`] returns. An
    /// ordinary route costs one read-locked probe and no write.
    pub fn on_route_deleted(&self, key: &str) -> Vec<String> {
        let (participant, naming) = {
            let g = self.inner.read();
            let participant =
                g.targets_by_parent.contains_key(key) || g.declared_by_child.contains_key(key) || g.roots.contains(key);
            let naming: Vec<String> = g.named_parents.get(key).into_iter().flatten().cloned().collect();
            (participant, naming)
        };
        let mut requeue: BTreeSet<String> = naming.into_iter().collect();
        if participant {
            requeue.extend(self.remove(key));
        }
        requeue.remove(key);
        requeue.into_iter().collect()
    }

    pub fn clear(&self) {
        *self.inner.write() = Inner::default();
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/delegation_graph_test.rs"]
mod tests;
