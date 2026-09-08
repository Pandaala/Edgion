use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use edgion_resources::ListenerKey;

/// Shared orchestration for L4 (TCP/TLS/UDP) `rebuild_all_listener_managers`.
///
/// # Invariants
///
/// 1. **PGIS pre-create**: per-listener managers are created for every active
///    listener known to `ListenerGatewayInfoStore`, even those without routes,
///    so `listener_builder` always finds a stable `Arc<M>` to attach.
///
/// 2. **Stale-sweep**: `by_listener` entries that have no routes AND are no
///    longer active in PGIS are dropped.
///
/// 3. **Clear-pass**: `by_listener` entries that are still active in PGIS but
///    have no routes after this rebuild get their route table re-emitted as an
///    empty snapshot (via `rebuild_one(manager, &empty_map)`). Without this
///    pass, the manager's `ArcSwap` retains routes from the previous rebuild
///    and the data plane keeps forwarding to deleted backends. Mirrors the
///    HTTP canonical implementation (`http/routes_mgr.rs`).
///
/// Parameters:
///   `active_listener_keys` — current `ListenerGatewayInfoStore` snapshot; the
///       caller fetches it (typically `pgis.all_listener_keys()`) and passes it
///       in so the helper is unit-testable without touching global PGIS state.
///   `get_listeners`  — extract resolved ListenerKeys from a cached route
///   `get_or_create`  — `get_or_create_listener_manager(&ListenerKey) -> Arc<M>`
///   `rebuild_one`    — `manager.rebuild(&HashMap<String, Arc<R>>)`
pub(crate) fn rebuild_all_l4_listener_managers<R, M>(
    component: &'static str,
    by_listener: &DashMap<ListenerKey, Arc<M>>,
    route_cache: &DashMap<String, Arc<R>>,
    active_listener_keys: Vec<ListenerKey>,
    get_listeners: impl Fn(&Arc<R>) -> Vec<ListenerKey>,
    get_or_create: impl Fn(&ListenerKey) -> Arc<M>,
    rebuild_one: impl Fn(&Arc<M>, &HashMap<String, Arc<R>>),
) {
    // Pre-create per-listener managers for every active listener so
    // `listener_builder` always finds a stable Arc<M> to attach.
    for key in &active_listener_keys {
        get_or_create(key);
    }
    let active_listeners: HashSet<ListenerKey> = active_listener_keys.into_iter().collect();

    // Bucket all cached routes by their resolved listeners.
    let listener_buckets = bucket_routes_by_listener(route_cache, &get_listeners, component);

    let mut rebuilt_listeners = 0u32;
    for (key, routes) in &listener_buckets {
        let manager = get_or_create(key);
        rebuild_one(&manager, routes);
        rebuilt_listeners += 1;
    }

    // Stale-sweep: drop entries that have no routes AND are not active in PGIS.
    let stale_listeners: Vec<ListenerKey> = by_listener
        .iter()
        .filter(|e| !listener_buckets.contains_key(e.key()) && !active_listeners.contains(e.key()))
        .map(|e| e.key().clone())
        .collect();
    let empty: HashMap<String, Arc<R>> = HashMap::new();
    let mut removed_stale = 0usize;
    for key in &stale_listeners {
        if let Some((_, manager)) = by_listener.remove(key) {
            rebuild_one(&manager, &empty);
            removed_stale += 1;
        }
    }

    // Clear-pass: for active listeners that no longer have any routes, emit an
    // empty snapshot so the data plane stops forwarding to deleted backends.
    // (TCP/TLS/UDP `*RouteTable::from_routes(empty)` is equivalent to `::new()`.)
    let mut cleared_active = 0u32;
    for entry in by_listener.iter() {
        let key = entry.key();
        if !listener_buckets.contains_key(key) {
            rebuild_one(entry.value(), &empty);
            cleared_active += 1;
        }
    }

    let total_routes: usize = listener_buckets.values().map(|r| r.len()).sum();
    tracing::info!(
        component = component,
        listeners = listener_buckets.len(),
        total_route_entries = total_routes,
        rebuilt_listeners,
        cleared_active_listeners = cleared_active,
        removed_stale_listeners = removed_stale,
        "Rebuilt all per-listener L4 route managers"
    );
}

/// Shared orchestration for L4 (TCP/TLS/UDP) `rebuild_affected_listener_managers`.
///
/// Single-pass: iterates `route_cache` once, bucketing routes into only the
/// `affected_listeners`. Listeners in `affected_listeners` that end up with no
/// routes are rebuilt with an empty set (clearing their snapshot) and removed
/// from `by_listener` if also absent from PGIS.
///
/// Also sweeps orphan entries: a partial update only touches affected
/// listeners, but earlier `rebuild_all` calls may have pre-created
/// `by_listener` entries for listeners that no longer appear in PGIS and are
/// not referenced by any current route. Without this sweep, those entries
/// (and their stale route tables) leak indefinitely — they are never the
/// target of a subsequent partial update.
// The 8 parameters fall into three orthogonal groups (caller context, PGIS
// snapshot + affected set, per-family closures). Bundling them into a struct
// would push the noise onto every call site without improving readability of
// the body, so we take the lint instead.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebuild_affected_l4_listener_managers<R, M>(
    component: &'static str,
    by_listener: &DashMap<ListenerKey, Arc<M>>,
    route_cache: &DashMap<String, Arc<R>>,
    affected_listeners: &HashSet<ListenerKey>,
    active_listener_keys: Vec<ListenerKey>,
    get_listeners: impl Fn(&Arc<R>) -> Vec<ListenerKey>,
    get_or_create: impl Fn(&ListenerKey) -> Arc<M>,
    rebuild_one: impl Fn(&Arc<M>, &HashMap<String, Arc<R>>),
) {
    if affected_listeners.is_empty() {
        return;
    }

    let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<R>>> = HashMap::new();
    for key in affected_listeners {
        listener_buckets.insert(key.clone(), HashMap::new());
    }

    for entry in route_cache.iter() {
        let route_listeners = get_listeners(entry.value());
        for key in route_listeners {
            if let Some(bucket) = listener_buckets.get_mut(&key) {
                bucket.insert(entry.key().clone(), entry.value().clone());
            }
        }
    }

    let active_listeners: HashSet<ListenerKey> = active_listener_keys.into_iter().collect();
    let mut removed_stale = 0usize;
    for (key, routes) in &listener_buckets {
        let manager = get_or_create(key);
        rebuild_one(&manager, routes);
        if routes.is_empty() && !active_listeners.contains(key) {
            if by_listener.remove(key).is_some() {
                removed_stale += 1;
            }
        }
    }

    // Orphan-sweep over by_listener entries that are not referenced by any
    // current route and are no longer active in PGIS.
    let route_referenced: HashSet<ListenerKey> = route_cache.iter().flat_map(|e| get_listeners(e.value())).collect();
    let orphans: Vec<ListenerKey> = by_listener
        .iter()
        .filter(|e| !route_referenced.contains(e.key()) && !active_listeners.contains(e.key()))
        .map(|e| e.key().clone())
        .collect();
    let empty: HashMap<String, Arc<R>> = HashMap::new();
    for key in &orphans {
        if let Some((_, manager)) = by_listener.remove(key) {
            rebuild_one(&manager, &empty);
            removed_stale += 1;
        }
    }

    tracing::info!(
        component = component,
        affected = affected_listeners.len(),
        removed_stale_listeners = removed_stale,
        "Rebuilt affected per-listener L4 route managers"
    );
}

/// Bucket all cached routes by their resolved listeners. Routes with no
/// resolved listeners are skipped with a `warn!` tagged by `component`.
fn bucket_routes_by_listener<R>(
    route_cache: &DashMap<String, Arc<R>>,
    get_listeners: &impl Fn(&Arc<R>) -> Vec<ListenerKey>,
    component: &'static str,
) -> HashMap<ListenerKey, HashMap<String, Arc<R>>> {
    let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<R>>> = HashMap::new();

    for entry in route_cache.iter() {
        let route_key = entry.key().clone();
        let route = entry.value().clone();
        let listeners = get_listeners(&route);

        if listeners.is_empty() {
            tracing::warn!(
                component = component,
                route = %route_key,
                "L4 route has no resolved_listeners, skipping listener assignment"
            );
            continue;
        }

        for key in listeners {
            listener_buckets
                .entry(key)
                .or_default()
                .insert(route_key.clone(), route.clone());
        }
    }

    listener_buckets
}
