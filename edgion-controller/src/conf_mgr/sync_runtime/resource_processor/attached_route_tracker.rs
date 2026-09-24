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

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

use edgion_resources::ResourceKind;

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

struct Inner {
    /// gateway_key → ListenerMap
    forward: HashMap<String, ListenerMap>,
    /// RouteRef → set of Attachments
    reverse: HashMap<RouteRef, HashSet<Attachment>>,
}

pub struct AttachedRouteTracker {
    inner: RwLock<Inner>,
}

impl Default for AttachedRouteTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl AttachedRouteTracker {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                forward: HashMap::new(),
                reverse: HashMap::new(),
            }),
        }
    }

    /// Register (or re-register) a route's attachments derived from its parentRefs.
    ///
    /// Removes old entries and inserts new ones atomically under a single write lock.
    /// Returns true if the attachments actually changed (or were newly created).
    pub fn update_route(&self, route_kind: ResourceKind, route_key: &str, attachments: HashSet<Attachment>) -> bool {
        let route_ref = RouteRef {
            kind: route_kind,
            key: route_key.to_string(),
        };

        let mut inner = self.inner.write();

        let old = inner.reverse.get(&route_ref);
        if old.map(|o| o == &attachments).unwrap_or(attachments.is_empty()) {
            return false;
        }

        let inner = &mut *inner;
        Self::remove_from_maps(&mut inner.forward, &mut inner.reverse, &route_ref);

        if attachments.is_empty() {
            return true;
        }

        for att in &attachments {
            inner
                .forward
                .entry(att.gateway_key.clone())
                .or_default()
                .entry(att.listener_name.clone())
                .or_default()
                .entry(route_kind)
                .or_default()
                .insert(route_key.to_string());
        }

        inner.reverse.insert(route_ref, attachments);
        true
    }

    /// Remove all entries for a deleted route.
    /// Returns true if the route was tracked (had entries to remove).
    pub fn remove_route(&self, route_kind: ResourceKind, route_key: &str) -> bool {
        let route_ref = RouteRef {
            kind: route_kind,
            key: route_key.to_string(),
        };

        let mut inner = self.inner.write();
        let had_entries = inner.reverse.contains_key(&route_ref);
        let inner = &mut *inner;
        Self::remove_from_maps(&mut inner.forward, &mut inner.reverse, &route_ref);
        had_entries
    }

    /// Count attached routes for a specific listener on a gateway.
    ///
    /// Returns: routes explicitly targeting `listener_name` + routes with no
    /// sectionName (targeting all listeners).
    pub fn count_for_listener(&self, gateway_key: &str, listener_name: &str) -> i32 {
        let inner = self.inner.read();
        let Some(listener_map) = inner.forward.get(gateway_key) else {
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

    /// Shared removal logic, called while the write lock is already held.
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
        let mut inner = self.inner.write();
        inner.forward.clear();
        inner.reverse.clear();
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/attached_route_tracker_test.rs"]
mod tests;
