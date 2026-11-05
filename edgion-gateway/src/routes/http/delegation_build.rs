//! HTTPRoute delegation: subtree build and child publication routing.
//!
//! A delegation child is never bucketed by listener. Its rules are compiled
//! once into units (`Arc`-shared across every key it holds), and each
//! delegation slot holds a `RouteRules` subtree assembled from the units of
//! all current children of that key. A child change rebuilds exactly the slots
//! of its added/removed/kept keys; listener tables are left untouched.
//!
//! Everything here runs on the cold publication path under the HTTP route
//! rebuild lock. With no delegation in the payload no slot is created and the
//! slot table stays empty; `full_set` still scans each route's rules once to
//! find delegation keys and children (cold path, linear in the payload).

use super::conf_handler_impl::{build_route_rules, compile_child_units};
use super::routes_mgr::{GlobalHttpRouteManagers, RouteRules};
use super::HttpRouteRuleUnit;
use crate::routes::common::accepted_route_kind_parents;
use crate::routes::delegation::index::DelegationChildIndex;
use crate::routes::delegation::{accept_child_keys, DelegationSlotTable, RouteParentView};
use edgion_resources::resources::route_delegation::{
    classify_parent_refs, delegation_target_of_http_rule, key_for, DelegationKey, ParentRefShape, RouteKindName,
};
use edgion_resources::HTTPRoute;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Compiled units of one child: (radix units, regex units).
type ChildUnits = (Vec<Arc<HttpRouteRuleUnit>>, Vec<Arc<HttpRouteRuleUnit>>);

/// Gateway-side HTTPRoute child index (see `DelegationChildIndex`).
pub(crate) type HttpDelegationIndex = DelegationChildIndex<ChildUnits>;

/// Assemble the subtree of `key` from the cached units of its children.
fn subtree(index: &HttpDelegationIndex, key: &DelegationKey) -> Option<Arc<RouteRules>> {
    let mut units = ChildUnits::default();
    for child in index.units_of(key) {
        units.0.extend(child.0.iter().cloned());
        units.1.extend(child.1.iter().cloned());
    }
    build_route_rules("delegation-subtree", units)
}

/// Every Gateway in the process as sorted `(namespace, name)` pairs: the set
/// a delegation child's leaf metric handles are prebuilt for (spec §8.3).
pub(crate) fn process_gateway_identities() -> Vec<(String, String)> {
    crate::runtime::store::get_global_gateway_store()
        .read()
        .gateway_identities()
}

/// Whether the route's parentRefs make it a delegation child (all refs point
/// at HTTPRoutes). Such a route has no listeners and is never bucketed.
pub(crate) fn is_delegation_child(route: &HTTPRoute) -> bool {
    matches!(
        classify_parent_refs(route.parent_refs(), route.route_ns(), RouteKindName::HTTPRoute),
        ParentRefShape::Child(_)
    )
}

/// Keys this child may publish into: Controller-resolved, locally confirmed
/// against the child's own parentRefs and shape, and honoring the Controller's
/// per-parentRef `Accepted` status like any ordinary route attachment.
pub(crate) fn child_accepted_keys(route: &HTTPRoute) -> Vec<DelegationKey> {
    if route.spec.resolved_delegation_keys.is_empty() {
        return Vec::new();
    }
    let mut keys = accept_child_keys(route, &route.spec.resolved_delegation_keys);
    if keys.is_empty() {
        return keys;
    }
    let accepted = accepted_route_kind_parents(
        route.spec.parent_refs.as_deref(),
        route.spec.current_status.as_ref().map(|s| s.parents.as_slice()),
        route.route_ns(),
        route.spec.resolved_status_controller.as_deref(),
    );
    keys.retain(|k| {
        accepted
            .iter()
            .any(|(ns, name)| *ns == k.parent_ns && *name == k.parent_name)
    });
    let mut seen = HashSet::new();
    keys.retain(|k| seen.insert(k.clone()));
    keys
}

/// Keys this route's own delegating rules point at (its parent side).
fn parent_delegation_keys(route: &HTTPRoute, out: &mut Vec<DelegationKey>) {
    let ns = route.route_ns();
    let Some(name) = route.metadata.name.as_deref() else {
        return;
    };
    for rule in route.spec.rules.as_ref().into_iter().flatten() {
        if let Some(Ok(target)) = delegation_target_of_http_rule(rule, ns) {
            out.push(key_for(RouteKindName::HTTPRoute, ns, name, &target));
        }
    }
}

/// Build one subtree from the given children: the same radix + regex
/// construction as a hostname group, each child compiled with its own
/// namespace/identity, no hostname and no listener/Gateway check. Leaf metric
/// handles are prebuilt for `process_gateways`.
pub fn build_subtree(
    children: &[Arc<HTTPRoute>],
    slots: &DelegationSlotTable<RouteRules>,
    process_gateways: &[(String, String)],
) -> Option<Arc<RouteRules>> {
    let mut units = ChildUnits::default();
    for child in children {
        let (radix, regex) = compile_child_units(child, slots, process_gateways);
        units.0.extend(radix);
        units.1.extend(regex);
    }
    build_route_rules("delegation-subtree", units)
}

impl GlobalHttpRouteManagers {
    /// The HTTPRoute delegation slot table.
    pub fn delegation_slots(&self) -> &DelegationSlotTable<RouteRules> {
        &self.delegation_slots
    }

    /// Store the current subtree of each touched key.
    fn store_subtrees(&self, index: &HttpDelegationIndex, keys: &HashSet<DelegationKey>) {
        for key in keys {
            let subtree = subtree(index, key);
            self.delegation_slots.get_or_create(key).store(subtree);
        }
    }

    /// Rebuild every slot from scratch for a full payload. Returns the keys to
    /// garbage-collect once the previous route generation is released.
    /// The caller MUST hold the rebuild lock.
    pub(crate) fn rebuild_all_delegation_locked<'a>(
        &self,
        previous: impl Iterator<Item = &'a Arc<HTTPRoute>>,
        current: &HashMap<String, Arc<HTTPRoute>>,
        process_gateways: impl FnOnce() -> Vec<(String, String)>,
    ) -> Vec<DelegationKey> {
        let mut index = self.delegation_index.lock();
        let mut touched: HashSet<DelegationKey> = index.key_children.keys().cloned().collect();
        let mut gc = Vec::new();
        for route in previous {
            parent_delegation_keys(route, &mut gc);
        }
        // Ordinary-only payloads never read the Gateway store.
        let has_children = current
            .values()
            .any(|route| !route.spec.resolved_delegation_keys.is_empty());
        *index = HttpDelegationIndex {
            gateways: if has_children { process_gateways() } else { Vec::new() },
            ..HttpDelegationIndex::default()
        };
        for (route_key, route) in current {
            parent_delegation_keys(route, &mut gc);
            let keys = child_accepted_keys(route);
            if keys.is_empty() {
                continue;
            }
            touched.extend(keys.iter().cloned());
            let units = compile_child_units(route, &self.delegation_slots, &index.gateways);
            index.link(route_key, keys, units);
        }
        self.store_subtrees(&index, &touched);
        gc.extend(touched);
        gc
    }

    /// Apply child publications from a partial update: rebuild exactly the
    /// slots of each changed child's added/removed/kept keys. Listener tables
    /// are not touched. `changed` pairs each changed route key with its
    /// previous generation; the new generation is read from the route cache.
    /// Returns the keys to garbage-collect. The caller MUST hold the rebuild lock.
    pub(crate) fn apply_delegation_changes_locked(
        &self,
        changed: &[(String, Option<Arc<HTTPRoute>>)],
        process_gateways: impl FnOnce() -> Vec<(String, String)>,
    ) -> Vec<DelegationKey> {
        let mut gc = Vec::new();
        let mut touched: HashSet<DelegationKey> = HashSet::new();
        let mut index = self.delegation_index.lock();
        // Keep every cached child on one Gateway set before linking new ones.
        // Ordinary-only updates (no cached child, no changed route carrying
        // delegation keys) never read the Gateway store.
        let involves_children = !index.children.is_empty()
            || changed.iter().any(|(route_key, _)| {
                self.route_cache
                    .get(route_key)
                    .is_some_and(|route| !route.spec.resolved_delegation_keys.is_empty())
            });
        if involves_children {
            touched.extend(self.recompile_children_for_gateways(&mut index, process_gateways()));
        }
        for (route_key, previous) in changed {
            let current = self.route_cache.get(route_key).map(|entry| Arc::clone(entry.value()));
            if let Some(previous) = previous {
                parent_delegation_keys(previous, &mut gc);
            }
            if let Some(current) = &current {
                parent_delegation_keys(current, &mut gc);
            }
            let new_keys = current.as_deref().map(child_accepted_keys).unwrap_or_default();
            if new_keys.is_empty() && !index.children.contains_key(route_key) {
                continue;
            }
            touched.extend(index.unlink(route_key));
            if let Some(current) = current.filter(|_| !new_keys.is_empty()) {
                touched.extend(new_keys.iter().cloned());
                let units = compile_child_units(&current, &self.delegation_slots, &index.gateways);
                index.link(route_key, new_keys, units);
            }
        }
        self.store_subtrees(&index, &touched);
        gc.extend(touched);
        gc
    }

    /// Recompile every cached child and restore its slots when the process
    /// Gateway set changed since the children were compiled, so leaf metric
    /// handles cover exactly the current Gateways. Called on a Gateway
    /// add/remove. A no-op when the set is unchanged or no child is cached
    /// (ordinary-only processes). Takes the rebuild lock.
    pub fn refresh_delegation_gateways(&self) {
        let _guard = self.lock_rebuild();
        if self.delegation_index.lock().children.is_empty() {
            // Nothing cached; the next linked child reads the current set.
            return;
        }
        self.refresh_delegation_gateways_locked(process_gateway_identities());
    }

    /// `refresh_delegation_gateways` body with an explicit Gateway set. The
    /// caller MUST hold the rebuild lock.
    pub(crate) fn refresh_delegation_gateways_locked(&self, process_gateways: Vec<(String, String)>) {
        let mut index = self.delegation_index.lock();
        let touched = self.recompile_children_for_gateways(&mut index, process_gateways);
        self.store_subtrees(&index, &touched);
    }

    /// Record `process_gateways` on the index; when it differs from the set
    /// the cached children were compiled with, recompile them all and return
    /// every key they hold (whose subtrees must be re-stored). The key set is
    /// unchanged, so no slot needs garbage collection.
    fn recompile_children_for_gateways(
        &self,
        index: &mut HttpDelegationIndex,
        process_gateways: Vec<(String, String)>,
    ) -> HashSet<DelegationKey> {
        if index.gateways == process_gateways {
            return HashSet::new();
        }
        index.gateways = process_gateways;
        if index.children.is_empty() {
            return HashSet::new();
        }
        let mut touched = HashSet::new();
        for (route_key, entry) in index.children.iter_mut() {
            let Some(route) = self.route_cache.get(route_key).map(|e| Arc::clone(e.value())) else {
                continue;
            };
            entry.units = compile_child_units(&route, &self.delegation_slots, &index.gateways);
            touched.extend(entry.keys.iter().cloned());
        }
        touched
    }

    /// Drop the given slots when nobody references them any more. Key-scoped:
    /// never scans the slot table.
    pub(crate) fn collect_delegation_slots(&self, keys: &[DelegationKey]) {
        self.delegation_slots.retain_referenced(keys);
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/http/delegation_build_test.rs"]
mod tests;
