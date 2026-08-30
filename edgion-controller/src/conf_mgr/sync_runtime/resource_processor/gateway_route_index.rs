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
//! All four maps (forward, reverse, gateway_hostnames, gateway_ports) live
//! under a single `RwLock<Inner>` so that listener-config change detection
//! and the subsequent route collection observe a consistent snapshot. This
//! closes a race where a concurrent `update_route` could insert a new route
//! into `forward` between the change-detection write and the route lookup,
//! causing the new route to miss its requeue.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

use edgion_resources::resources::common::ParentReference;
use edgion_resources::ResourceKind;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteEntry {
    kind: ResourceKind,
    key: String,
}

#[derive(Default)]
struct Inner {
    forward: HashMap<String, HashSet<RouteEntry>>,
    reverse: HashMap<RouteEntry, HashSet<String>>,
    /// Cached sorted listener hostnames per gateway, used for change detection
    /// so that route requeue only fires when listener hostnames actually change.
    gateway_hostnames: HashMap<String, Vec<String>>,
    /// Cached sorted listener ports per gateway for change detection.
    /// Port changes must requeue TLSRoute/EdgionTls that reference sectionName.
    gateway_ports: HashMap<String, Vec<i32>>,
}

impl Inner {
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

        let mut gateway_keys = HashSet::new();
        for pr in parent_refs {
            let parent_group = pr.group.as_deref().unwrap_or("gateway.networking.k8s.io");
            let parent_kind = pr.kind.as_deref().unwrap_or("Gateway");
            if parent_group != "gateway.networking.k8s.io" || parent_kind != "Gateway" {
                continue;
            }
            let gw_key = pr.build_parent_key(Some(route_ns));
            gateway_keys.insert(gw_key);
        }

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

/// Result of an atomic listener-config update returned by
/// [`GatewayRouteIndex::update_listener_config_and_collect_routes`].
///
/// Only constructed when at least one of `hostnames_changed` / `ports_changed`
/// is true; callers use the flags for telemetry and `routes` for the requeue
/// fan-out.
#[derive(Debug)]
pub struct ListenerConfigChange {
    pub hostnames_changed: bool,
    pub ports_changed: bool,
    pub routes: Vec<(ResourceKind, String)>,
}

pub struct GatewayRouteIndex {
    inner: RwLock<Inner>,
}

impl GatewayRouteIndex {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
        }
    }

    /// Update the cached listener hostnames for a Gateway.
    /// Returns true if the hostnames actually changed (i.e., routes need requeue).
    pub fn update_gateway_hostnames(&self, gateway_key: &str, mut hostnames: Vec<String>) -> bool {
        hostnames.sort();
        let mut g = self.inner.write();
        match g.gateway_hostnames.get(gateway_key) {
            Some(old) if *old == hostnames => false,
            _ => {
                g.gateway_hostnames.insert(gateway_key.to_string(), hostnames);
                true
            }
        }
    }

    /// Remove cached hostnames for a deleted Gateway.
    pub fn remove_gateway_hostnames(&self, gateway_key: &str) {
        self.inner.write().gateway_hostnames.remove(gateway_key);
    }

    /// Update the cached listener ports for a Gateway.
    /// Returns true if the ports actually changed (i.e., routes need requeue).
    pub fn update_gateway_ports(&self, gateway_key: &str, mut ports: Vec<i32>) -> bool {
        ports.sort();
        let mut g = self.inner.write();
        match g.gateway_ports.get(gateway_key) {
            Some(old) if *old == ports => false,
            _ => {
                g.gateway_ports.insert(gateway_key.to_string(), ports);
                true
            }
        }
    }

    /// Remove cached ports for a deleted Gateway.
    pub fn remove_gateway_ports(&self, gateway_key: &str) {
        self.inner.write().gateway_ports.remove(gateway_key);
    }

    /// Atomically update both listener hostnames and ports for a Gateway, and
    /// — if either changed — collect the routes that reference this Gateway
    /// under the same write guard.
    ///
    /// Returns:
    /// - `None` when neither hostnames nor ports changed (caller skips requeue).
    /// - `Some(ListenerConfigChange { hostnames_changed, ports_changed, routes })`
    ///   when at least one changed; `routes` is the snapshot of the forward
    ///   index for this Gateway, observed atomically with the hostnames/ports
    ///   write. `routes` may be empty if no route currently references the
    ///   Gateway. The `*_changed` flags are exposed for telemetry so callers
    ///   can log which axis triggered the requeue.
    ///
    /// This closes the race where a concurrent `update_route` could insert a
    /// new route into `forward` between change detection and route lookup.
    pub fn update_listener_config_and_collect_routes(
        &self,
        gateway_key: &str,
        mut hostnames: Vec<String>,
        mut ports: Vec<i32>,
    ) -> Option<ListenerConfigChange> {
        hostnames.sort();
        ports.sort();

        let mut g = self.inner.write();

        let hostnames_changed = match g.gateway_hostnames.get(gateway_key) {
            Some(old) if *old == hostnames => false,
            _ => {
                g.gateway_hostnames.insert(gateway_key.to_string(), hostnames);
                true
            }
        };

        let ports_changed = match g.gateway_ports.get(gateway_key) {
            Some(old) if *old == ports => false,
            _ => {
                g.gateway_ports.insert(gateway_key.to_string(), ports);
                true
            }
        };

        if !hostnames_changed && !ports_changed {
            return None;
        }

        Some(ListenerConfigChange {
            hostnames_changed,
            ports_changed,
            routes: g.collect_routes_for_gateway(gateway_key),
        })
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
    /// Test-only: no production caller today.
    #[cfg(test)]
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
        g.gateway_hostnames.clear();
        g.gateway_ports.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(ns: &str, name: &str) -> ParentReference {
        ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("Gateway".to_string()),
            namespace: Some(ns.to_string()),
            name: name.to_string(),
            section_name: None,
            port: None,
        }
    }

    #[test]
    fn test_basic_registration() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw1")],
            "default",
        );

        let routes = idx.get_routes_for_gateway("default/gw1");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0], (ResourceKind::HTTPRoute, "default/r1".to_string()));
    }

    #[test]
    fn test_multi_gateway() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw1"), pr("other", "gw2")],
            "default",
        );

        assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 1);
        assert_eq!(idx.get_routes_for_gateway("other/gw2").len(), 1);
    }

    #[test]
    fn test_update_replaces() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw1")],
            "default",
        );
        assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 1);

        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw2")],
            "default",
        );
        assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 0);
        assert_eq!(idx.get_routes_for_gateway("default/gw2").len(), 1);
    }

    #[test]
    fn test_remove() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw1")],
            "default",
        );
        assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 1);

        idx.remove_route(ResourceKind::HTTPRoute, "default/r1");
        assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 0);
    }

    #[test]
    fn test_multiple_routes_same_gateway() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw1")],
            "default",
        );
        idx.update_route(
            ResourceKind::GRPCRoute,
            "default/g1",
            &[pr("default", "gw1")],
            "default",
        );

        let routes = idx.get_routes_for_gateway("default/gw1");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn test_non_gateway_parent_ref_ignored() {
        let idx = GatewayRouteIndex::new();
        let non_gw_ref = ParentReference {
            group: Some("example.io".to_string()),
            kind: Some("Service".to_string()),
            namespace: Some("default".to_string()),
            name: "svc1".to_string(),
            section_name: None,
            port: None,
        };
        idx.update_route(ResourceKind::HTTPRoute, "default/r1", &[non_gw_ref], "default");

        assert!(idx.get_routes_for_gateway("default/svc1").is_empty());
    }

    #[test]
    fn test_get_gateways_for_route() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw1"), pr("other", "gw2")],
            "default",
        );

        let mut gateways = idx.get_gateways_for_route(ResourceKind::HTTPRoute, "default/r1");
        gateways.sort();
        assert_eq!(gateways, vec!["default/gw1", "other/gw2"]);

        // Non-existent route returns empty
        let empty = idx.get_gateways_for_route(ResourceKind::HTTPRoute, "default/nonexistent");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_atomic_listener_update_no_change() {
        let idx = GatewayRouteIndex::new();
        // Seed with current state
        idx.update_gateway_hostnames("default/gw1", vec!["a.example.com".into()]);
        idx.update_gateway_ports("default/gw1", vec![80]);

        // Same values → None
        let result =
            idx.update_listener_config_and_collect_routes("default/gw1", vec!["a.example.com".into()], vec![80]);
        assert!(result.is_none());
    }

    #[test]
    fn test_atomic_listener_update_hostnames_changed() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw1")],
            "default",
        );
        idx.update_gateway_hostnames("default/gw1", vec!["old.example.com".into()]);
        idx.update_gateway_ports("default/gw1", vec![80]);

        let result =
            idx.update_listener_config_and_collect_routes("default/gw1", vec!["new.example.com".into()], vec![80]);
        let change = result.expect("hostnames changed → should return Some");
        assert!(change.hostnames_changed);
        assert!(!change.ports_changed);
        assert_eq!(change.routes.len(), 1);
        assert_eq!(change.routes[0], (ResourceKind::HTTPRoute, "default/r1".to_string()));
    }

    #[test]
    fn test_atomic_listener_update_ports_changed() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(ResourceKind::TLSRoute, "default/t1", &[pr("default", "gw1")], "default");
        idx.update_gateway_hostnames("default/gw1", vec!["a.example.com".into()]);
        idx.update_gateway_ports("default/gw1", vec![443]);

        let result =
            idx.update_listener_config_and_collect_routes("default/gw1", vec!["a.example.com".into()], vec![443, 8443]);
        let change = result.expect("ports changed → should return Some");
        assert!(!change.hostnames_changed);
        assert!(change.ports_changed);
        assert_eq!(change.routes.len(), 1);
        assert_eq!(change.routes[0], (ResourceKind::TLSRoute, "default/t1".to_string()));
    }

    #[test]
    fn test_atomic_listener_update_first_call_returns_some() {
        // First call against a fresh index: both hostnames and ports go from
        // absent to present → both "changed" → returns Some.
        let idx = GatewayRouteIndex::new();
        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw1")],
            "default",
        );

        let result =
            idx.update_listener_config_and_collect_routes("default/gw1", vec!["a.example.com".into()], vec![80]);
        let change = result.expect("first call should be Some");
        assert!(change.hostnames_changed);
        assert!(change.ports_changed);
        assert_eq!(change.routes.len(), 1);

        // Second call with same values → None
        let result2 =
            idx.update_listener_config_and_collect_routes("default/gw1", vec!["a.example.com".into()], vec![80]);
        assert!(result2.is_none());
    }

    #[test]
    fn test_atomic_listener_update_no_referencing_routes() {
        let idx = GatewayRouteIndex::new();
        // Gateway has no referencing routes yet
        let result =
            idx.update_listener_config_and_collect_routes("default/gw1", vec!["a.example.com".into()], vec![80]);
        let change = result.expect("first call → Some");
        assert!(change.hostnames_changed);
        assert!(change.ports_changed);
        assert!(change.routes.is_empty());
    }

    #[test]
    fn update_route_returns_dropped_gateways() {
        let index = GatewayRouteIndex::new();

        let dropped = index.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw-a")],
            "default",
        );
        assert!(dropped.is_empty(), "first registration drops nothing");

        // Re-point the route from gw-a to gw-b: gw-a must be reported dropped.
        let dropped = index.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw-b")],
            "default",
        );
        assert_eq!(dropped, vec!["default/gw-a".to_string()]);

        // Unchanged refs drop nothing.
        let dropped = index.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw-b")],
            "default",
        );
        assert!(dropped.is_empty());
    }

    #[test]
    fn test_atomic_listener_update_input_order_irrelevant() {
        let idx = GatewayRouteIndex::new();
        idx.update_listener_config_and_collect_routes(
            "default/gw1",
            vec!["b.example.com".into(), "a.example.com".into()],
            vec![443, 80],
        );

        // Same set, different order → no change
        let result = idx.update_listener_config_and_collect_routes(
            "default/gw1",
            vec!["a.example.com".into(), "b.example.com".into()],
            vec![80, 443],
        );
        assert!(result.is_none());
    }
}
