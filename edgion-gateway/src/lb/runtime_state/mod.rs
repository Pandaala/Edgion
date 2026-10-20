//! Service-scoped LB runtime state — SP affinity + backend lifecycle ledger.
//!
//! Two responsibilities:
//!   1. Session-persistence affinity map (token → endpoint SocketAddr).
//!   2. Backend lifecycle ledger: draining registry.
//!
//! Metric atomics (the packed Peak-EWMA `score`, `conn_count`) live on `LbBackend` itself.
//! `DRAINING_BACKENDS` holds the live `Arc<LbBackend>` so `BackendCleaner`
//! can poll `backend.owned_conn()` without a side counter.

use crate::backends::discovery::ServiceSectionKey;
use dashmap::DashMap;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::sync::{Arc, LazyLock};

use super::backend::LbBackend;

// ── SP affinity map ───────────────────────────────────────────────────────────

/// Max entries per service in the session affinity map.
const SP_AFFINITY_MAX_PER_SERVICE: usize = 1024;

/// Membership-tagged sticky snapshot owned by one Service runtime cell.
///
/// The coordinator publishes this only after revalidating both the joint
/// Service generation and the active runtime membership.
pub struct SpAffinitySnapshot {
    pub membership_identity: u64,
    pub entries: Arc<std::collections::HashMap<String, std::net::SocketAddr>>,
}

fn build_sp_affinity_map<'a>(
    backends: impl ExactSizeIterator<Item = &'a LbBackend>,
    scope: Option<&str>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
    for b in backends.take(SP_AFFINITY_MAX_PER_SERVICE) {
        if let SocketAddr::Inet(addr) = b.addr {
            let addr_str = addr.to_string();
            // Threat model: this unkeyed hash of `IP:port` (no secret, no MAC)
            // is a stickiness hint, NOT a security boundary. A client that
            // knows a backend's `IP:port` can forge the matching token to pin
            // its own requests to that backend, but the map only ever holds the
            // service's real backends, so a forged token can only pick an
            // existing backend (never an arbitrary address) and only affects
            // the forging client's own traffic. See
            // `session_persistence::generate_session_token` (the single token
            // derivation — map keys and response-injected tokens must never
            // drift) for the full rationale. Use an HMAC over a secret if
            // affinity is ever promoted to a routing restriction.
            let token = crate::routes::http::session_persistence::generate_scoped_session_token(scope, &addr_str);
            map.insert(token, addr);
        }
    }
    Arc::new(map)
}

/// Get or lazily build the session affinity map. The `backends_fn` closure is
/// only invoked on a cache miss, avoiding unnecessary allocation on the hot path.
pub(crate) fn sp_affinity_get_or_build_key(
    service_key: &str,
    section: &ServiceSectionKey,
    _backends_fn: impl FnOnce() -> Arc<[Arc<LbBackend>]>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    let coordinator = crate::backends::discovery::get_service_discovery_coordinator();
    sp_affinity_get_or_build_from(coordinator, service_key, section)
}

fn sp_affinity_get_or_build_from(
    coordinator: &crate::backends::discovery::ServiceDiscoveryCoordinator,
    service_key: &str,
    section: &ServiceSectionKey,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    let generation = coordinator.load_generation();
    let Some(available) = generation.available(service_key) else {
        return Arc::new(std::collections::HashMap::new());
    };
    let membership = available.membership_identity;
    let cell = &available.runtime_cell;
    let active = cell.active.load_full();
    if active.membership_identity != Some(membership) {
        return Arc::new(std::collections::HashMap::new());
    }
    if let Some(snapshot) = cell.sp.load_full().get(section) {
        if snapshot.membership_identity == membership {
            return Arc::clone(&snapshot.entries);
        }
    }

    let _build_guard = cell.sp_build_lock.lock();
    if let Some(snapshot) = cell.sp.load_full().get(section) {
        if snapshot.membership_identity == membership {
            return Arc::clone(&snapshot.entries);
        }
    }
    try_publish_sp_snapshot(coordinator, service_key, section, &generation, &active, available, cell)
        .unwrap_or_else(|| Arc::new(std::collections::HashMap::new()))
}

fn try_publish_sp_snapshot(
    coordinator: &crate::backends::discovery::ServiceDiscoveryCoordinator,
    _service_key: &str,
    section: &ServiceSectionKey,
    generation: &Arc<crate::backends::discovery::ServiceSelectorGeneration>,
    active: &Arc<crate::backends::discovery::ServiceRuntimeSnapshot>,
    available: &Arc<crate::backends::discovery::AvailableServiceProjection>,
    cell: &Arc<crate::backends::discovery::ServiceRuntimeCell>,
) -> Option<Arc<std::collections::HashMap<String, std::net::SocketAddr>>> {
    let membership = available.membership_identity;
    let current_generation = coordinator.load_generation();
    let current_active = cell.active.load_full();
    let valid = Arc::ptr_eq(generation, &current_generation)
        && Arc::ptr_eq(active, &current_active)
        && current_active.membership_identity == Some(membership);
    if !valid {
        return None;
    }
    let resolved = available.resolve_section(section).ok()?;
    let entries = build_sp_affinity_map(resolved.all_slots.iter().map(|slot| slot.backend.as_ref()), None);
    let after_generation = coordinator.load_generation();
    let after_active = cell.active.load_full();
    if !Arc::ptr_eq(&current_generation, &after_generation)
        || !Arc::ptr_eq(&current_active, &after_active)
        || after_active.membership_identity != Some(membership)
    {
        return None;
    }
    let mut next = (*cell.sp.load_full()).clone();
    next.insert(
        section.clone(),
        Arc::new(SpAffinitySnapshot {
            membership_identity: membership,
            entries: Arc::clone(&entries),
        }),
    );
    cell.sp.store(Arc::new(next));
    Some(entries)
}

pub(crate) fn sp_affinity_get_or_build_scoped(
    service_key: &str,
    section: &ServiceSectionKey,
    scope: Option<&str>,
    backends_fn: impl FnOnce() -> Arc<[Arc<LbBackend>]>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    // Typed scopes may include a direct backend UID. Keeping those generations in
    // this process-global Service cache would let an in-flight request recreate
    // a retired scope after hot-reload cleanup. Build the bounded view from the
    // request-captured endpoint Arc instead; only ordinary Service affinity is
    // cached globally.
    match scope {
        Some(scope) => {
            let backends = backends_fn();
            build_sp_affinity_map(backends.iter().map(Arc::as_ref), Some(scope))
        }
        None => sp_affinity_get_or_build_key(service_key, section, backends_fn),
    }
}

// ── Backend lifecycle ledger ──────────────────────────────────────────────────

/// Draining backend registry: backends removed from EP/EPS that still have
/// in-flight connections. Holds the live `Arc<LbBackend>` so
/// `BackendCleaner` can poll the exact wrapper's generation-owned connection
/// count. Shared transport counters are never used as drain ownership.
/// Key: `"service_key\0addr_string\0incarnation\0proof"` for typed backends.
static DRAINING_BACKENDS: LazyLock<DashMap<String, Arc<LbBackend>>> = LazyLock::new(DashMap::new);

pub struct DrainingStats {
    pub backend_count: usize,
    pub service_keys: Vec<String>,
    pub service_keys_truncated: bool,
}

pub fn draining_stats(key_limit: usize) -> DrainingStats {
    let backend_count = DRAINING_BACKENDS.len();
    let mut service_keys = DRAINING_BACKENDS
        .iter()
        .filter_map(|entry| entry.key().split('\0').next().map(str::to_string))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let service_keys_truncated = service_keys.len() > key_limit;
    service_keys.truncate(key_limit);
    DrainingStats {
        backend_count,
        service_keys,
        service_keys_truncated,
    }
}

fn backend_key(service_key: &str, addr: &SocketAddr, backend: &LbBackend) -> String {
    match backend.runtime_identity() {
        Some(identity) => format!(
            "{}\0{}\0{}\0{}",
            service_key,
            addr,
            identity.incarnation,
            hex::encode(identity.proof_digest)
        ),
        None => format!("{}\0{}", service_key, addr),
    }
}

/// Register a backend as draining. Call when a backend is removed from EP/EPS
/// but still has in-flight connections. The cleaner polls `backend.owned_conn()` and
/// removes it once it reaches zero.
pub fn begin_draining(service_key: &str, addr: &SocketAddr, backend: Arc<LbBackend>) {
    let key = backend_key(service_key, addr, &backend);
    DRAINING_BACKENDS.insert(key, backend);
    tracing::info!(
        service_key = %service_key,
        "Backend marked as draining"
    );
}

/// Remove lifecycle state for exactly one backend identity.
pub fn remove_backend_exact(service_key: &str, addr: &SocketAddr, backend: &LbBackend) {
    DRAINING_BACKENDS.remove(&backend_key(service_key, addr, backend));
}

pub fn finish_draining(key: &str) {
    DRAINING_BACKENDS.remove(key);
}

/// Return all backends currently in draining state, as `(key, Arc<LbBackend>)`.
/// Used by `BackendCleaner` to poll conn counts.
pub fn all_draining_backends() -> Vec<(String, Arc<LbBackend>)> {
    DRAINING_BACKENDS
        .iter()
        .map(|e| (e.key().clone(), e.value().clone()))
        .collect()
}

/// Remove the entire lifecycle state for a service (service deleted).
///
/// Concurrency window (documented, not fixed): `retain()` is a compound
/// multi-key scan-and-delete that can interleave with concurrent per-key
/// writers — `begin_draining` / `remove_backend` driven by the EndpointSlice
/// dispatch task, and `remove_backend` driven by the background
/// `BackendCleaner`. A `begin_draining` insert landing in a DashMap shard
/// that `retain` has already scanned can survive this sweep, leaving a
/// draining backend whose owning service has been removed.
///
/// Accepted because DRAINING_BACKENDS is self-healing runtime lifecycle
/// state — `BackendCleaner` (see `lb/leastconn/cleaner.rs`) removes any
/// backend whose `owned_conn()` reaches zero. The orphan is bounded by connection
/// lifetime, not a permanent residue. See
/// `docs/superpowers/specs/2026-06-09-gateway-cross-kind-derived-state-race-audit-design.md`
/// for the full audit.
pub fn remove_service(service_key: &str) {
    let prefix = format!("{}\0", service_key);
    DRAINING_BACKENDS.retain(|k, _| !k.starts_with(&prefix));
    tracing::info!(
        service_key = %service_key,
        "Removed service runtime state"
    );
}

#[cfg(test)]
#[path = "../../../tests/unit/lb/runtime_state/mod_test.rs"]
mod tests;
