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

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, RwLock};

use crate::types::resources::common::ParentReference;
use crate::types::ResourceKind;

static GATEWAY_ROUTE_INDEX: LazyLock<GatewayRouteIndex> = LazyLock::new(GatewayRouteIndex::new);

pub fn get_gateway_route_index() -> &'static GatewayRouteIndex {
    &GATEWAY_ROUTE_INDEX
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteEntry {
    kind: ResourceKind,
    key: String,
}

pub struct GatewayRouteIndex {
    forward: RwLock<HashMap<String, HashSet<RouteEntry>>>,
    reverse: RwLock<HashMap<RouteEntry, HashSet<String>>>,
    /// Cached sorted listener hostnames per gateway, used for change detection
    /// so that route requeue only fires when listener hostnames actually change.
    gateway_hostnames: RwLock<HashMap<String, Vec<String>>>,
}

impl GatewayRouteIndex {
    fn new() -> Self {
        Self {
            forward: RwLock::new(HashMap::new()),
            reverse: RwLock::new(HashMap::new()),
            gateway_hostnames: RwLock::new(HashMap::new()),
        }
    }

    /// Update the cached listener hostnames for a Gateway.
    /// Returns true if the hostnames actually changed (i.e., routes need requeue).
    pub fn update_gateway_hostnames(&self, gateway_key: &str, mut hostnames: Vec<String>) -> bool {
        hostnames.sort();
        let mut cache = self.gateway_hostnames.write().unwrap();
        match cache.get(gateway_key) {
            Some(old) if *old == hostnames => false,
            _ => {
                cache.insert(gateway_key.to_string(), hostnames);
                true
            }
        }
    }

    /// Remove cached hostnames for a deleted Gateway.
    pub fn remove_gateway_hostnames(&self, gateway_key: &str) {
        self.gateway_hostnames.write().unwrap().remove(gateway_key);
    }

    /// Update the index for a route based on its parentRefs.
    ///
    /// Uses ALL parentRefs (not filtered by Accepted status) because
    /// hostname resolution needs to happen for all referenced Gateways.
    pub fn update_route(
        &self,
        route_kind: ResourceKind,
        route_key: &str,
        parent_refs: &[ParentReference],
        route_ns: &str,
    ) {
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

        let mut fwd = self.forward.write().unwrap();
        let mut rev = self.reverse.write().unwrap();

        // Remove old entries
        if let Some(old_gw_keys) = rev.remove(&entry) {
            for old_gw_key in &old_gw_keys {
                if let Some(routes) = fwd.get_mut(old_gw_key) {
                    routes.remove(&entry);
                    if routes.is_empty() {
                        fwd.remove(old_gw_key);
                    }
                }
            }
        }

        if gateway_keys.is_empty() {
            return;
        }

        // Insert new entries
        for gw_key in &gateway_keys {
            fwd.entry(gw_key.clone()).or_default().insert(entry.clone());
        }
        rev.insert(entry, gateway_keys);
    }

    /// Remove all entries for a deleted route.
    pub fn remove_route(&self, route_kind: ResourceKind, route_key: &str) {
        let entry = RouteEntry {
            kind: route_kind,
            key: route_key.to_string(),
        };

        let mut fwd = self.forward.write().unwrap();
        let mut rev = self.reverse.write().unwrap();

        if let Some(old_gw_keys) = rev.remove(&entry) {
            for old_gw_key in &old_gw_keys {
                if let Some(routes) = fwd.get_mut(old_gw_key) {
                    routes.remove(&entry);
                    if routes.is_empty() {
                        fwd.remove(old_gw_key);
                    }
                }
            }
        }
    }

    /// Get all routes that reference a given Gateway.
    ///
    /// Returns (ResourceKind, route_key) pairs for requeue.
    pub fn get_routes_for_gateway(&self, gateway_key: &str) -> Vec<(ResourceKind, String)> {
        let fwd = self.forward.read().unwrap();
        match fwd.get(gateway_key) {
            Some(entries) => entries.iter().map(|e| (e.kind, e.key.clone())).collect(),
            None => Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn clear(&self) {
        self.forward.write().unwrap().clear();
        self.reverse.write().unwrap().clear();
        self.gateway_hostnames.write().unwrap().clear();
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
}
