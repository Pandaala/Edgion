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

    fn gateway(listeners: serde_json::Value) -> Gateway {
        serde_json::from_value(serde_json::json!({
            "metadata": {"name": "gw1", "namespace": "default"},
            "spec": {"gatewayClassName": "test", "listeners": listeners}
        }))
        .unwrap()
    }

    fn listener(name: &str, hostname: Option<&str>, port: i32) -> serde_json::Value {
        serde_json::json!({"name": name, "hostname": hostname, "port": port, "protocol": "TLS"})
    }

    #[test]
    fn test_atomic_listener_update_no_change() {
        let idx = GatewayRouteIndex::new();
        let gateway = gateway(serde_json::json!([listener("a", Some("a.example.com"), 443)]));
        assert!(idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .is_some());
        assert!(idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .is_none());
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
        let mut gateway = gateway(serde_json::json!([listener("a", Some("old.example.com"), 443)]));
        idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
        gateway.spec.listeners.as_mut().unwrap()[0].hostname = Some("new.example.com".into());
        let routes = idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .unwrap();
        assert_eq!(routes, [(ResourceKind::HTTPRoute, "default/r1".to_string())]);
    }

    #[test]
    fn test_atomic_listener_update_ports_changed() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(ResourceKind::TLSRoute, "default/t1", &[pr("default", "gw1")], "default");
        let mut gateway = gateway(serde_json::json!([listener("a", Some("a.example.com"), 443)]));
        idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
        gateway.spec.listeners.as_mut().unwrap()[0].port = 8443;
        let routes = idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .unwrap();
        assert_eq!(routes, [(ResourceKind::TLSRoute, "default/t1".to_string())]);
    }

    #[test]
    fn test_atomic_listener_update_first_call_returns_some() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw1")],
            "default",
        );
        let gateway = gateway(serde_json::json!([listener("a", None, 443)]));
        let routes = idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .unwrap();
        assert_eq!(routes, [(ResourceKind::HTTPRoute, "default/r1".to_string())]);
        assert!(idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .is_none());
    }

    #[test]
    fn test_atomic_listener_update_no_referencing_routes() {
        let idx = GatewayRouteIndex::new();
        let gateway = gateway(serde_json::json!([]));
        assert_eq!(
            idx.update_listener_config_and_collect_routes("default/gw1", &gateway),
            Some(vec![])
        );
        assert!(idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .is_none());
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

        // Pre-register before lookup without dropping old edges before status.
        index.register_route_dependencies(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw-b")],
            "default",
        );
        assert_eq!(index.get_routes_for_gateway("default/gw-a").len(), 1);
        assert_eq!(index.get_routes_for_gateway("default/gw-b").len(), 1);
        // Re-point after status: gw-a must still be reported dropped.
        let dropped = index.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            &[pr("default", "gw-b")],
            "default",
        );
        assert_eq!(dropped, vec!["default/gw-a".to_string()]);
        assert!(index.get_routes_for_gateway("default/gw-a").is_empty());

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
        let mut gateway = gateway(serde_json::json!([
            listener("a", Some("a.example.com"), 443),
            listener("b", Some("b.example.com"), 8443)
        ]));
        idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
        gateway.spec.listeners.as_mut().unwrap().reverse();
        assert!(idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .is_none());
    }

    #[test]
    fn attachment_inputs_preserve_associations_and_policy_changes() {
        let base = serde_json::json!([listener("a", Some("a.example.com"), 443), listener("b", None, 443)]);
        for (pointer, value) in [
            ("/0/name", serde_json::json!("renamed")),
            ("/0/protocol", serde_json::json!("tls")),
            ("/0/port", serde_json::json!(443 + 65536)),
            (
                "/0/allowedRoutes",
                serde_json::json!({"kinds": [{"group": "other.test", "kind": "TLSRoute"}]}),
            ),
            (
                "/0/allowedRoutes",
                serde_json::json!({"namespaces": {"from": "Selector", "selector": {"matchLabels": {"team": "blue"}}}}),
            ),
        ] {
            let idx = GatewayRouteIndex::new();
            let original = gateway(base.clone());
            idx.update_listener_config_and_collect_routes("default/gw1", &original);
            let mut changed = base.clone();
            if pointer.ends_with("allowedRoutes") {
                changed[0]["allowedRoutes"] = value;
            } else {
                *changed.pointer_mut(pointer).unwrap() = value;
            }
            assert!(
                idx.update_listener_config_and_collect_routes("default/gw1", &gateway(changed))
                    .is_some(),
                "{pointer}"
            );
        }
        let idx = GatewayRouteIndex::new();
        let mut gateway = gateway(base);
        idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
        let listeners = gateway.spec.listeners.as_mut().unwrap();
        listeners[1].hostname = listeners[0].hostname.take();
        assert!(idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .is_some());
        gateway.spec.addresses = Some(vec![serde_json::from_value(
            serde_json::json!({"type": "IPAddress", "value": "127.0.0.1"}),
        )
        .unwrap()]);
        assert!(idx
            .update_listener_config_and_collect_routes("default/gw1", &gateway)
            .is_some());
    }

    #[test]
    fn removing_listener_config_keeps_edges_for_recreation_and_clear_resets_all() {
        let idx = GatewayRouteIndex::new();
        idx.update_route(ResourceKind::TLSRoute, "default/r1", &[pr("default", "gw1")], "default");
        let gateway = gateway(serde_json::json!([listener("a", None, 443)]));
        idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
        let removed = idx.remove_listener_config_and_collect_routes("default/gw1");
        assert_eq!(removed, [(ResourceKind::TLSRoute, "default/r1".to_string())]);
        assert_eq!(
            idx.update_listener_config_and_collect_routes("default/gw1", &gateway),
            Some(removed)
        );
        idx.clear();
        assert!(idx.get_routes_for_gateway("default/gw1").is_empty());
        assert_eq!(
            idx.update_listener_config_and_collect_routes("default/gw1", &gateway),
            Some(vec![])
        );
    }
}
