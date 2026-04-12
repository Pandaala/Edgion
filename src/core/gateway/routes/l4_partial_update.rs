use crate::types::ListenerKey;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Shared orchestration for L4 (TCP/TLS/UDP) `partial_update`.
///
/// # Invariants
///
/// 1. **Complete pre-collection**: old listeners are collected for *all* keys
///    (`remove ∪ update ∪ add`) before any cache mutation — handles the case
///    where an existing key is erroneously placed in the `add` bucket.
///
/// 2. **Operation order**: `remove → update → add` — "vacate before occupy"
///    semantics, consistent with TCP/UDP convention.
// Parameters:
//   get_existing    — look up an existing (cached) route by key
//   get_listeners   — extract resolved ListenerKeys from a cached route
//   remove_cached   — remove a route from the cache
//   init_and_insert — initialize a new route and insert it into the cache;
//                     returns the resolved ListenerKeys on success
//   rebuild         — rebuild only the listener managers that were affected
pub(crate) fn apply_l4_partial_update<R>(
    route_kind: &'static str,
    add: HashMap<String, R>,
    update: HashMap<String, R>,
    remove: HashSet<String>,
    get_existing: impl Fn(&str) -> Option<Arc<R>>,
    get_listeners: impl Fn(&Arc<R>) -> Vec<ListenerKey>,
    remove_cached: impl Fn(&str),
    init_and_insert: impl Fn(R) -> Result<Vec<ListenerKey>, String>,
    rebuild: impl Fn(&HashSet<ListenerKey>),
) {
    let mut affected: HashSet<ListenerKey> = HashSet::new();

    // ── Pre-collect old listeners before any cache mutation ───────────────────
    for key in remove.iter().chain(update.keys()).chain(add.keys()) {
        if let Some(existing) = get_existing(key) {
            for lk in get_listeners(&existing) {
                affected.insert(lk);
            }
        }
    }

    // ── Phase 1: remove ───────────────────────────────────────────────────────
    for key in remove.iter() {
        remove_cached(key);
        tracing::debug!(resource_key = %key, kind = route_kind, "route removed");
    }

    // ── Phase 2: update (remove old, insert new) ──────────────────────────────
    for (key, route) in update {
        remove_cached(&key);
        match init_and_insert(route) {
            Ok(new_lks) => {
                affected.extend(new_lks);
                tracing::debug!(resource_key = %key, kind = route_kind, "route updated");
            }
            Err(e) => {
                tracing::error!(
                    resource_key = %key,
                    error = %e,
                    kind = route_kind,
                    "route update failed, old version removed"
                );
            }
        }
    }

    // ── Phase 3: add ──────────────────────────────────────────────────────────
    for (key, route) in add {
        match init_and_insert(route) {
            Ok(new_lks) => {
                affected.extend(new_lks);
                tracing::debug!(resource_key = %key, kind = route_kind, "route added");
            }
            Err(e) => {
                tracing::error!(
                    resource_key = %key,
                    error = %e,
                    kind = route_kind,
                    "route add failed"
                );
            }
        }
    }

    rebuild(&affected);
}
