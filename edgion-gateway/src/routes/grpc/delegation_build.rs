//! GRPCRoute delegation: subtree build and child publication routing.
//!
//! Mirrors `routes::http::delegation_build`. A delegation child is never
//! bucketed by listener. Its rules are compiled once into units (`Arc`-shared
//! across every key it holds), and each delegation slot holds a
//! `GrpcMatchEngine` assembled from the units of all current children of that
//! key. A child change rebuilds exactly the slots of its added/removed/kept
//! keys; listener tables are left untouched.
//!
//! Everything here runs on the cold publication path under the gRPC route
//! rebuild lock. With no delegation in the payload no slot is created and the
//! slot table stays empty; `full_set` still scans each route's rules once to
//! find delegation keys and children (cold path, linear in the payload).

use super::conf_handler_impl::compile_child_units;
use super::routes_mgr::GlobalGrpcRouteManagers;
use super::{GrpcMatchEngine, GrpcRouteRuleUnit};
use crate::routes::common::accepted_route_kind_parents;
use crate::routes::delegation::index::DelegationChildIndex;
use crate::routes::delegation::{accept_child_keys, DelegationSlotTable, RouteParentView};
use edgion_resources::resources::route_delegation::{
    classify_parent_refs, delegation_target_of_grpc_rule, key_for, DelegationKey, ParentRefShape, RouteKindName,
};
use edgion_resources::GRPCRoute;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Compiled units of one child.
type ChildUnits = Vec<Arc<GrpcRouteRuleUnit>>;

/// Gateway-side GRPCRoute child index (see `DelegationChildIndex`).
pub(crate) type GrpcDelegationIndex = DelegationChildIndex<ChildUnits>;

/// Build a subtree engine from child units; `None` when there are none (or
/// the engine cannot be built, which fails closed: the slot matches nothing).
fn engine_from_units(units: ChildUnits) -> Option<Arc<GrpcMatchEngine>> {
    if units.is_empty() {
        return None;
    }
    let count = units.len();
    match GrpcMatchEngine::rebuild(units, None) {
        Ok(engine) => Some(Arc::new(engine)),
        Err(error) => {
            tracing::error!(
                component = "grpc_route_delegation",
                route_count = count,
                err = %error,
                "Failed to build a GRPCRoute delegation subtree; the slot matches nothing"
            );
            None
        }
    }
}

/// Assemble the subtree of `key` from the cached units of its children.
fn subtree(index: &GrpcDelegationIndex, key: &DelegationKey) -> Option<Arc<GrpcMatchEngine>> {
    engine_from_units(index.units_of(key).flatten().cloned().collect())
}

/// Whether the route's parentRefs make it a delegation child (all refs point
/// at GRPCRoutes). Such a route has no listeners and is never bucketed.
pub(crate) fn is_grpc_delegation_child(route: &GRPCRoute) -> bool {
    matches!(
        classify_parent_refs(route.parent_refs(), route.route_ns(), RouteKindName::GRPCRoute),
        ParentRefShape::Child(_)
    )
}

/// Keys this child may publish into: Controller-resolved, locally confirmed
/// against the child's own parentRefs and shape, and honoring the Controller's
/// per-parentRef `Accepted` status like any ordinary route attachment.
pub(crate) fn grpc_child_accepted_keys(route: &GRPCRoute) -> Vec<DelegationKey> {
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
fn parent_delegation_keys(route: &GRPCRoute, out: &mut Vec<DelegationKey>) {
    let ns = route.route_ns();
    let Some(name) = route.metadata.name.as_deref() else {
        return;
    };
    for rule in route.spec.rules.as_ref().into_iter().flatten() {
        if let Some(Ok(target)) = delegation_target_of_grpc_rule(rule, ns) {
            out.push(key_for(RouteKindName::GRPCRoute, ns, name, &target));
        }
    }
}

/// Build one subtree engine from the given children, each compiled with its
/// own namespace/identity, no hostname and no listener/Gateway check. Leaf
/// metric handles are prebuilt for `process_gateways`.
pub fn build_grpc_subtree(
    children: &[Arc<GRPCRoute>],
    slots: &DelegationSlotTable<GrpcMatchEngine>,
    process_gateways: &[(String, String)],
) -> Option<Arc<GrpcMatchEngine>> {
    engine_from_units(
        children
            .iter()
            .flat_map(|child| compile_child_units(child, slots, process_gateways))
            .collect(),
    )
}

impl GlobalGrpcRouteManagers {
    /// The GRPCRoute delegation slot table.
    pub fn delegation_slots(&self) -> &DelegationSlotTable<GrpcMatchEngine> {
        &self.delegation_slots
    }

    /// Store the current subtree of each touched key.
    fn store_subtrees(&self, index: &GrpcDelegationIndex, keys: &HashSet<DelegationKey>) {
        for key in keys {
            self.delegation_slots.get_or_create(key).store(subtree(index, key));
        }
    }

    /// Rebuild every slot from scratch for a full payload. Returns the keys to
    /// garbage-collect once the previous route generation is released.
    /// The caller MUST hold the rebuild lock.
    pub(crate) fn rebuild_all_delegation_locked<'a>(
        &self,
        previous: impl Iterator<Item = &'a Arc<GRPCRoute>>,
        current: &HashMap<String, Arc<GRPCRoute>>,
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
        *index = GrpcDelegationIndex {
            gateways: if has_children { process_gateways() } else { Vec::new() },
            ..GrpcDelegationIndex::default()
        };
        for (route_key, route) in current {
            parent_delegation_keys(route, &mut gc);
            let keys = grpc_child_accepted_keys(route);
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
        changed: &[(String, Option<Arc<GRPCRoute>>)],
        process_gateways: impl FnOnce() -> Vec<(String, String)>,
    ) -> Vec<DelegationKey> {
        let mut gc = Vec::new();
        let mut touched: HashSet<DelegationKey> = HashSet::new();
        let mut index = self.delegation_index.lock();
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
            let new_keys = current.as_deref().map(grpc_child_accepted_keys).unwrap_or_default();
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
    /// Gateway set changed, so leaf metric handles cover exactly the current
    /// Gateways. A no-op when no child is cached. Takes the rebuild lock.
    pub fn refresh_delegation_gateways(&self) {
        let _guard = self.lock_rebuild();
        if self.delegation_index.lock().children.is_empty() {
            return;
        }
        self.refresh_delegation_gateways_locked(crate::routes::http::delegation_build::process_gateway_identities());
    }

    /// `refresh_delegation_gateways` body with an explicit Gateway set. The
    /// caller MUST hold the rebuild lock.
    pub(crate) fn refresh_delegation_gateways_locked(&self, process_gateways: Vec<(String, String)>) {
        let mut index = self.delegation_index.lock();
        let touched = self.recompile_children_for_gateways(&mut index, process_gateways);
        self.store_subtrees(&index, &touched);
    }

    fn recompile_children_for_gateways(
        &self,
        index: &mut GrpcDelegationIndex,
        process_gateways: Vec<(String, String)>,
    ) -> HashSet<DelegationKey> {
        if index.gateways == process_gateways {
            return HashSet::new();
        }
        index.gateways = process_gateways;
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

    /// Drop the given slots when nobody references them any more. Key-scoped.
    pub(crate) fn collect_delegation_slots(&self, keys: &[DelegationKey]) {
        self.delegation_slots.retain_referenced(keys);
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/grpc/delegation_build_test.rs"]
mod tests;
