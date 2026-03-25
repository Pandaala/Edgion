//! Attached Route Tracker
//!
//! Maintains a global index of which routes reference which Gateway listeners
//! via their parentRefs. Updated by route handlers on change/delete, queried
//! by the Gateway handler during status computation.
//!
//! ## Data Model
//!
//! Forward index (two-level):
//!   gateway_key (`"{gw_ns}/{gw_name}"`)
//!     → listener_name (`"http"` | `""` for unspecified sectionName)
//!       → `ResourceKind` → `{route_keys}`
//!
//! Reverse index:
//!   `RouteRef` → `{Attachment}`
//!
//! ## Why Route Doesn't Need to Query Gateway
//!
//! A Route's parentRef already contains (gateway ns, gateway name, optional
//! sectionName).  This is enough to update the tracker — no Gateway lookup
//! needed.  Routes that arrive before their target Gateway simply pre-populate
//! the index; when the Gateway eventually arrives its `update_status` reads the
//! correct counts.
//!
//! ## Counting at Read Time
//!
//! `count_for_listener(gw_key, "http")` = routes targeting listener `"http"` +
//! routes targeting `""` (meaning "all listeners on this gateway").

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, RwLock};

use crate::types::ResourceKind;

static ATTACHED_ROUTE_TRACKER: LazyLock<AttachedRouteTracker> = LazyLock::new(AttachedRouteTracker::new);

pub fn get_attached_route_tracker() -> &'static AttachedRouteTracker {
    &ATTACHED_ROUTE_TRACKER
}

/// Identifies a specific route resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteRef {
    pub kind: ResourceKind,
    /// `"{ns}/{name}"`
    pub key: String,
}

/// Identifies which gateway listener a route attaches to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Attachment {
    /// `"{gw_ns}/{gw_name}"`
    pub gateway_key: String,
    /// Listener name from parentRef.sectionName, or `""` for all listeners.
    pub listener_name: String,
}

/// Per-listener attachment map: `listener_name → { ResourceKind → { route_keys } }`.
type ListenerMap = HashMap<String, HashMap<ResourceKind, HashSet<String>>>;

pub struct AttachedRouteTracker {
    /// gateway_key → ListenerMap
    forward: RwLock<HashMap<String, ListenerMap>>,
    /// RouteRef → set of Attachments
    reverse: RwLock<HashMap<RouteRef, HashSet<Attachment>>>,
}

impl AttachedRouteTracker {
    fn new() -> Self {
        Self {
            forward: RwLock::new(HashMap::new()),
            reverse: RwLock::new(HashMap::new()),
        }
    }

    /// Register (or re-register) a route's attachments derived from its parentRefs.
    ///
    /// Single-lock-acquisition: removes old entries and inserts new ones atomically.
    /// Returns true if the attachments actually changed (or were newly created).
    pub fn update_route(&self, route_kind: ResourceKind, route_key: &str, attachments: HashSet<Attachment>) -> bool {
        let route_ref = RouteRef {
            kind: route_kind,
            key: route_key.to_string(),
        };

        let mut fwd = self.forward.write().unwrap();
        let mut rev = self.reverse.write().unwrap();

        let old = rev.get(&route_ref);
        if old.map(|o| o == &attachments).unwrap_or(attachments.is_empty()) {
            return false;
        }

        Self::remove_from_maps(&mut fwd, &mut rev, &route_ref);

        if attachments.is_empty() {
            return true;
        }

        for att in &attachments {
            fwd.entry(att.gateway_key.clone())
                .or_default()
                .entry(att.listener_name.clone())
                .or_default()
                .entry(route_kind)
                .or_default()
                .insert(route_key.to_string());
        }

        rev.insert(route_ref, attachments);
        true
    }

    /// Remove all entries for a deleted route.
    /// Returns true if the route was tracked (had entries to remove).
    pub fn remove_route(&self, route_kind: ResourceKind, route_key: &str) -> bool {
        let route_ref = RouteRef {
            kind: route_kind,
            key: route_key.to_string(),
        };

        let mut fwd = self.forward.write().unwrap();
        let mut rev = self.reverse.write().unwrap();

        let had_entries = rev.contains_key(&route_ref);
        Self::remove_from_maps(&mut fwd, &mut rev, &route_ref);
        had_entries
    }

    /// Count attached routes for a specific listener on a gateway.
    ///
    /// Returns: routes explicitly targeting `listener_name` + routes with no
    /// sectionName (targeting all listeners).
    pub fn count_for_listener(&self, gateway_key: &str, listener_name: &str) -> i32 {
        let fwd = self.forward.read().unwrap();
        let Some(listener_map) = fwd.get(gateway_key) else {
            return 0;
        };

        let mut total: i32 = 0;

        if let Some(kinds) = listener_map.get(listener_name) {
            total += kinds.values().map(|r| r.len() as i32).sum::<i32>();
        }

        // Wildcard: routes with empty sectionName attach to all listeners
        if !listener_name.is_empty() {
            if let Some(kinds) = listener_map.get("") {
                total += kinds.values().map(|r| r.len() as i32).sum::<i32>();
            }
        }

        total
    }

    /// Shared removal logic, called while locks are already held.
    fn remove_from_maps(
        fwd: &mut HashMap<String, ListenerMap>,
        rev: &mut HashMap<RouteRef, HashSet<Attachment>>,
        route_ref: &RouteRef,
    ) {
        let Some(old_attachments) = rev.remove(route_ref) else {
            return;
        };

        for att in &old_attachments {
            let Some(listener_map) = fwd.get_mut(&att.gateway_key) else {
                continue;
            };
            let Some(kinds) = listener_map.get_mut(&att.listener_name) else {
                continue;
            };
            if let Some(routes) = kinds.get_mut(&route_ref.kind) {
                routes.remove(&route_ref.key);
                if routes.is_empty() {
                    kinds.remove(&route_ref.kind);
                }
            }
            if kinds.is_empty() {
                listener_map.remove(&att.listener_name);
            }
            if listener_map.is_empty() {
                fwd.remove(&att.gateway_key);
            }
        }
    }

    pub fn clear(&self) {
        self.forward.write().unwrap().clear();
        self.reverse.write().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(gw: &str, listener: &str) -> Attachment {
        Attachment {
            gateway_key: gw.to_string(),
            listener_name: listener.to_string(),
        }
    }

    #[test]
    fn test_explicit_listener() {
        let t = AttachedRouteTracker::new();
        t.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            HashSet::from([att("default/gw", "http")]),
        );

        assert_eq!(t.count_for_listener("default/gw", "http"), 1);
        assert_eq!(t.count_for_listener("default/gw", "https"), 0);
    }

    #[test]
    fn test_wildcard_listener() {
        let t = AttachedRouteTracker::new();
        t.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            HashSet::from([att("default/gw", "")]),
        );

        assert_eq!(t.count_for_listener("default/gw", "http"), 1);
        assert_eq!(t.count_for_listener("default/gw", "https"), 1);
        assert_eq!(t.count_for_listener("default/gw", ""), 1);
    }

    #[test]
    fn test_mixed_explicit_and_wildcard() {
        let t = AttachedRouteTracker::new();

        t.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            HashSet::from([att("default/gw", "http")]),
        );
        t.update_route(
            ResourceKind::TCPRoute,
            "default/t1",
            HashSet::from([att("default/gw", "")]),
        );

        assert_eq!(t.count_for_listener("default/gw", "http"), 2);
        assert_eq!(t.count_for_listener("default/gw", "https"), 1);
    }

    #[test]
    fn test_update_replaces_old() {
        let t = AttachedRouteTracker::new();

        t.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            HashSet::from([att("default/gw", "http")]),
        );
        assert_eq!(t.count_for_listener("default/gw", "http"), 1);

        t.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            HashSet::from([att("default/gw", "https")]),
        );
        assert_eq!(t.count_for_listener("default/gw", "http"), 0);
        assert_eq!(t.count_for_listener("default/gw", "https"), 1);
    }

    #[test]
    fn test_remove_route() {
        let t = AttachedRouteTracker::new();
        t.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            HashSet::from([att("default/gw", "http")]),
        );
        assert_eq!(t.count_for_listener("default/gw", "http"), 1);

        t.remove_route(ResourceKind::HTTPRoute, "default/r1");
        assert_eq!(t.count_for_listener("default/gw", "http"), 0);
    }

    #[test]
    fn test_multiple_routes_same_listener() {
        let t = AttachedRouteTracker::new();
        let a = HashSet::from([att("default/gw", "http")]);

        t.update_route(ResourceKind::HTTPRoute, "default/r1", a.clone());
        t.update_route(ResourceKind::HTTPRoute, "default/r2", a.clone());
        t.update_route(ResourceKind::GRPCRoute, "default/g1", a);

        assert_eq!(t.count_for_listener("default/gw", "http"), 3);
    }

    #[test]
    fn test_route_arrives_before_gateway() {
        let t = AttachedRouteTracker::new();

        t.update_route(
            ResourceKind::HTTPRoute,
            "default/r1",
            HashSet::from([att("default/gw", "http")]),
        );

        assert_eq!(t.count_for_listener("default/gw", "http"), 1);
    }

    #[test]
    fn test_route_multi_gateway() {
        let t = AttachedRouteTracker::new();

        t.update_route(
            ResourceKind::HTTPRoute,
            "ns-a/r1",
            HashSet::from([att("ns-a/gw-a", "http"), att("ns-b/gw-b", "https")]),
        );

        assert_eq!(t.count_for_listener("ns-a/gw-a", "http"), 1);
        assert_eq!(t.count_for_listener("ns-b/gw-b", "https"), 1);
        assert_eq!(t.count_for_listener("ns-a/gw-a", "https"), 0);
    }
}
