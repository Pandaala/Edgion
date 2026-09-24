//! Gateway → Route Index
//!
//! Maintains a reverse index from Gateway keys to referencing routes, built
//! from route parentRefs. When a Gateway's listeners change (especially
//! hostnames), this index is used to requeue affected routes so the controller
//! can re-resolve their effective hostnames.
//!
//! ## Data Model
//!
//! Forward index:
//!   gateway_key (`"{gw_ns}/{gw_name}"`) → Set<(ResourceKind, route_key)>
//!
//! Reverse index:
//!   (ResourceKind, route_key) → Set<gateway_key>
//!
//! Built from ALL parentRefs (not just accepted ones), because even pending
//! routes need to be re-resolved when a Gateway changes.
//!
//! ## Concurrency
//!
//! The dependency maps and listener configuration records live
//! under a single `RwLock<Inner>` so that listener-config change detection
//! and the subsequent route collection observe a consistent snapshot. This
//! closes a race where a concurrent `update_route` could insert a new route
//! into `forward` between the change-detection write and the route lookup,
//! causing the new route to miss its requeue.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

use edgion_resources::listener_key::resolve_listener_addr;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::gateway::AllowedRoutes;
use edgion_resources::{Gateway, ListenerKey, ResourceKind};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteEntry {
    kind: ResourceKind,
    key: String,
}

#[derive(Default)]
struct Inner {
    forward: HashMap<String, HashSet<RouteEntry>>,
    reverse: HashMap<RouteEntry, HashSet<String>>,
    /// Routing inputs, preserving each logical listener's field associations.
    gateway_listeners: HashMap<String, GatewayRouteConfig>,
}

#[derive(Default)]
struct GatewayRouteConfig {
    listeners: Option<Vec<ListenerRouteConfig>>,
    /// Stable incarnation identity, never the per-publication proof epoch.
    ownership: Option<(String, String)>,
}

impl Inner {
    fn gateway_keys(parent_refs: &[ParentReference], route_ns: &str) -> HashSet<String> {
        parent_refs
            .iter()
            .filter(|parent| {
                parent.group.as_deref().unwrap_or("gateway.networking.k8s.io") == "gateway.networking.k8s.io"
                    && parent.kind.as_deref().unwrap_or("Gateway") == "Gateway"
            })
            .map(|parent| parent.build_parent_key(Some(route_ns)))
            .collect()
    }

    fn collect_routes_for_gateway(&self, gateway_key: &str) -> Vec<(ResourceKind, String)> {
        match self.forward.get(gateway_key) {
            Some(entries) => entries.iter().map(|e| (e.kind, e.key.clone())).collect(),
            None => Vec::new(),
        }
    }

    /// Returns the gateway keys this route referenced before the update but no
    /// longer does — the caller must requeue them so their derived state
    /// (e.g. `attachedRoutes`) converges even when delete->create coalescing
    /// in the workqueue skips `handler.on_delete` for the previous generation.
    fn update_route_inner(
        &mut self,
        route_kind: ResourceKind,
        route_key: &str,
        parent_refs: &[ParentReference],
        route_ns: &str,
    ) -> Vec<String> {
        let entry = RouteEntry {
            kind: route_kind,
            key: route_key.to_string(),
        };

        let gateway_keys = Self::gateway_keys(parent_refs, route_ns);

        // Remove old entries, remembering which gateways this route used to
        // reference so the diff below can report the ones it no longer does.
        let old_gw_keys = self.reverse.remove(&entry).unwrap_or_default();
        for old_gw_key in &old_gw_keys {
            if let Some(routes) = self.forward.get_mut(old_gw_key) {
                routes.remove(&entry);
                if routes.is_empty() {
                    self.forward.remove(old_gw_key);
                }
            }
        }

        let mut dropped: Vec<String> = old_gw_keys.difference(&gateway_keys).cloned().collect();
        dropped.sort();

        if gateway_keys.is_empty() {
            return dropped;
        }

        for gw_key in &gateway_keys {
            self.forward.entry(gw_key.clone()).or_default().insert(entry.clone());
        }
        self.reverse.insert(entry, gateway_keys);

        dropped
    }

    fn remove_route_inner(&mut self, route_kind: ResourceKind, route_key: &str) {
        let entry = RouteEntry {
            kind: route_kind,
            key: route_key.to_string(),
        };

        if let Some(old_gw_keys) = self.reverse.remove(&entry) {
            for old_gw_key in &old_gw_keys {
                if let Some(routes) = self.forward.get_mut(old_gw_key) {
                    routes.remove(&entry);
                    if routes.is_empty() {
                        self.forward.remove(old_gw_key);
                    }
                }
            }
        }
    }
}

/// Only inputs consumed by route attachment resolution. Never retain TLS
/// credentials, publication proofs, or status in the requeue comparison.
#[derive(Debug, PartialEq)]
struct ListenerRouteConfig {
    name: String,
    hostname: Option<String>,
    listener_key: ListenerKey,
    port: i32,
    protocol: String,
    allowed_routes: Option<AllowedRoutes>,
}

impl ListenerRouteConfig {
    fn for_gateway(gateway: &Gateway) -> Vec<Self> {
        let mut listeners: Vec<_> = gateway
            .spec
            .listeners
            .iter()
            .flatten()
            .map(|listener| Self {
                name: listener.name.clone(),
                hostname: listener.hostname.clone(),
                listener_key: ListenerKey::new(
                    resolve_listener_addr(gateway.spec.addresses.as_ref(), listener.port),
                    &listener.protocol,
                ),
                port: listener.port,
                protocol: listener.protocol.clone(),
                allowed_routes: listener.allowed_routes.clone(),
            })
            .collect();
        listeners.sort_by(|a, b| a.name.cmp(&b.name));
        listeners
    }
}

pub struct GatewayRouteIndex {
    inner: RwLock<Inner>,
}

impl Default for GatewayRouteIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl GatewayRouteIndex {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
        }
    }

    /// Remove a deleted Gateway's routing inputs and collect dependents under
    /// the same lock. Keep dependency edges so recreation requeues them again.
    pub fn remove_listener_config_and_collect_routes(&self, gateway_key: &str) -> Vec<(ResourceKind, String)> {
        let mut inner = self.inner.write();
        inner.gateway_listeners.remove(gateway_key);
        inner.collect_routes_for_gateway(gateway_key)
    }

    /// Atomically compare the published Gateway's attachment inputs and collect
    /// dependents. None means no routing input changed; Some may be empty when
    /// no route references the Gateway yet. Unrelated publication changes must
    /// not trigger the Route -> Gateway -> Route feedback loop.
    pub fn update_listener_config_and_collect_routes(
        &self,
        gateway_key: &str,
        gateway: &Gateway,
    ) -> Option<Vec<(ResourceKind, String)>> {
        let listeners = ListenerRouteConfig::for_gateway(gateway);
        let mut inner = self.inner.write();
        let entry = inner.gateway_listeners.entry(gateway_key.to_string()).or_default();
        if entry.listeners.as_ref() == Some(&listeners) {
            return None;
        }
        entry.listeners = Some(listeners);
        Some(inner.collect_routes_for_gateway(gateway_key))
    }

    /// Record the published Gateway/class incarnation without feeding status or
    /// proof-epoch echoes back into the route queues. Clearing a lost proof is a
    /// real transition too. The record shares listener-config cleanup ownership.
    pub fn update_publication_ownership(&self, gateway_key: &str, ownership: Option<(String, String)>) -> bool {
        let mut inner = self.inner.write();
        let entry = inner.gateway_listeners.entry(gateway_key.to_string()).or_default();
        if entry.ownership == ownership {
            return false;
        }
        entry.ownership = ownership;
        true
    }

    /// Register new dependencies before reading Gateway snapshots. Keep old
    /// edges until update_route runs after status computation, so dropped
    /// Gateways are requeued only after their attachment counts are replaced.
    /// The caller must complete update_route or remove_route after this step.
    pub fn register_route_dependencies(
        &self,
        route_kind: ResourceKind,
        route_key: &str,
        parent_refs: &[ParentReference],
        route_ns: &str,
    ) {
        let keys = Inner::gateway_keys(parent_refs, route_ns);
        if keys.is_empty() {
            return;
        }
        let entry = RouteEntry {
            kind: route_kind,
            key: route_key.to_string(),
        };
        let mut inner = self.inner.write();
        for key in &keys {
            inner.forward.entry(key.clone()).or_default().insert(entry.clone());
        }
        inner.reverse.entry(entry).or_default().extend(keys);
    }

    /// Update the index for a route based on its parentRefs.
    ///
    /// Uses ALL parentRefs (not filtered by Accepted status) because
    /// hostname resolution needs to happen for all referenced Gateways.
    ///
    /// Returns the gateway keys the route referenced before this call but no
    /// longer does (computed atomically under the same write guard as the
    /// update) — the apply path must requeue them so their derived state
    /// (e.g. `attachedRoutes`) converges even when delete->create coalescing
    /// in the workqueue skips `handler.on_delete`.
    pub fn update_route(
        &self,
        route_kind: ResourceKind,
        route_key: &str,
        parent_refs: &[ParentReference],
        route_ns: &str,
    ) -> Vec<String> {
        self.inner
            .write()
            .update_route_inner(route_kind, route_key, parent_refs, route_ns)
    }

    /// Remove all entries for a deleted route.
    pub fn remove_route(&self, route_kind: ResourceKind, route_key: &str) {
        self.inner.write().remove_route_inner(route_kind, route_key);
    }

    /// Get all routes that reference a given Gateway.
    ///
    /// Returns (ResourceKind, route_key) pairs for requeue.
    pub fn get_routes_for_gateway(&self, gateway_key: &str) -> Vec<(ResourceKind, String)> {
        self.inner.read().collect_routes_for_gateway(gateway_key)
    }

    /// Get all Gateways that a given route references via parentRefs.
    ///
    /// Returns gateway keys (e.g., `"default/my-gateway"`).
    /// Used by BackendTLSPolicy to resolve ancestors:
    /// BTP.targetRef(Service) → ServiceRefManager → Route → this method → Gateway(s)
    pub fn get_gateways_for_route(&self, route_kind: ResourceKind, route_key: &str) -> Vec<String> {
        let entry = RouteEntry {
            kind: route_kind,
            key: route_key.to_string(),
        };
        let g = self.inner.read();
        match g.reverse.get(&entry) {
            Some(gw_keys) => gw_keys.iter().cloned().collect(),
            None => Vec::new(),
        }
    }

    /// Get all unique routes across all gateways.
    /// Used for post-init revalidation to ensure routes that registered
    /// after their parent Gateway's on_change are reprocessed.
    pub fn all_routes(&self) -> Vec<(ResourceKind, String)> {
        let g = self.inner.read();
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for entries in g.forward.values() {
            for e in entries {
                if seen.insert((&e.kind, &e.key)) {
                    result.push((e.kind, e.key.clone()));
                }
            }
        }
        result
    }

    pub fn clear(&self) {
        let mut g = self.inner.write();
        g.forward.clear();
        g.reverse.clear();
        g.gateway_listeners.clear();
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/gateway_route_index_test.rs"]
mod tests;
