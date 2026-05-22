//! Service-scoped LB runtime state — SP affinity + backend lifecycle ledger.
//!
//! Stage 3 reduced this module to two responsibilities:
//!   1. Session-persistence affinity map (token → endpoint SocketAddr).
//!   2. Backend lifecycle ledger: draining registry and circuit-breaker state.
//!
//! Metric atomics (`ewma_us`, `conn_count`) are now on `EdgionBackend` itself.
//! The old `SERVICES` DashMap (keyed by `(service_key, addr)`) is gone.
//! `DRAINING_BACKENDS` holds the live `Arc<EdgionBackend>` so `BackendCleaner`
//! can poll `backend.conn()` without a side counter.

use dashmap::DashMap;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::sync::{Arc, LazyLock};

use super::edgion_backend::EdgionBackend;

// ── SP affinity map ───────────────────────────────────────────────────────────

/// Session affinity map: token (CRC32 hex) → endpoint SocketAddr, per service key.
/// Built lazily on first session persistence request. Invalidated when backends change.
/// Cap: max 1024 entries per service to prevent memory explosion.
static SP_AFFINITY: LazyLock<DashMap<String, Arc<std::collections::HashMap<String, std::net::SocketAddr>>>> =
    LazyLock::new(DashMap::new);

/// Max entries per service in the session affinity map.
const SP_AFFINITY_MAX_PER_SERVICE: usize = 1024;

/// Drop the cached SP affinity map for a service (called when backends change
/// or the service is removed). Other selector caches now live on
/// `compiled_selector::ServiceSelectorCell` and follow that lifecycle.
pub fn invalidate_selector_cache(service_key: &str) {
    SP_AFFINITY.remove(service_key);
}

/// Look up a session token in the affinity map for a service.
/// Returns the endpoint SocketAddr if the token is known.
pub fn sp_affinity_lookup(service_key: &str, token: &str) -> Option<std::net::SocketAddr> {
    SP_AFFINITY.get(service_key).and_then(|map| map.get(token).copied())
}

fn build_sp_affinity_map(
    backends: &[Arc<EdgionBackend>],
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
    for b in backends.iter().take(SP_AFFINITY_MAX_PER_SERVICE) {
        if let SocketAddr::Inet(addr) = b.addr {
            let addr_str = addr.to_string();
            let token = format!("{:08x}", crc32fast::hash(addr_str.as_bytes()));
            map.insert(token, addr);
        }
    }
    Arc::new(map)
}

/// Build or refresh the session affinity map for a service from its current backends.
pub fn sp_affinity_build(service_key: &str, backends: &[Arc<EdgionBackend>]) {
    SP_AFFINITY.insert(service_key.to_string(), build_sp_affinity_map(backends));
}

/// Get or lazily build the session affinity map. The `backends_fn` closure is
/// only invoked on a cache miss, avoiding unnecessary allocation on the hot path.
pub(crate) fn sp_affinity_get_or_build_key(
    service_key: &str,
    backends_fn: impl FnOnce() -> Arc<Vec<Arc<EdgionBackend>>>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    SP_AFFINITY
        .entry(service_key.to_string())
        .or_insert_with(|| build_sp_affinity_map(&backends_fn()))
        .value()
        .clone()
}

// ── Backend lifecycle ledger ──────────────────────────────────────────────────

/// Draining backend registry: backends removed from EP/EPS that still have
/// in-flight connections. Holds the live `Arc<EdgionBackend>` so
/// `BackendCleaner` can poll `backend.conn()` without a separate counter.
/// Key: `"service_key\0addr_string"`.
static DRAINING_BACKENDS: LazyLock<DashMap<String, Arc<EdgionBackend>>> = LazyLock::new(DashMap::new);

fn backend_key(service_key: &str, addr: &SocketAddr) -> String {
    format!("{}\0{}", service_key, addr)
}

/// Register a backend as draining. Call when a backend is removed from EP/EPS
/// but still has in-flight connections. The cleaner polls `backend.conn()` and
/// removes it once it reaches zero.
pub fn begin_draining(service_key: &str, addr: &SocketAddr, backend: Arc<EdgionBackend>) {
    let key = backend_key(service_key, addr);
    DRAINING_BACKENDS.insert(key, backend);
    tracing::info!(
        service_key = %service_key,
        backend = %addr,
        "Backend marked as draining"
    );
}

/// Remove all lifecycle state for a backend (called when fully drained or
/// when a backend disappears cleanly with no in-flight connections).
pub fn remove_backend(service_key: &str, addr: &SocketAddr) {
    let key = backend_key(service_key, addr);
    DRAINING_BACKENDS.remove(&key);
}

/// Return all backends currently in draining state, as `(key, Arc<EdgionBackend>)`.
/// Used by `BackendCleaner` to poll conn counts.
pub fn all_draining_backends() -> Vec<(String, Arc<EdgionBackend>)> {
    DRAINING_BACKENDS
        .iter()
        .map(|e| (e.key().clone(), e.value().clone()))
        .collect()
}

/// Remove the entire lifecycle state for a service (service deleted).
pub fn remove_service(service_key: &str) {
    let prefix = format!("{}\0", service_key);
    DRAINING_BACKENDS.retain(|k, _| !k.starts_with(&prefix));
    invalidate_selector_cache(service_key);
    tracing::info!(
        service_key = %service_key,
        "Removed service runtime state"
    );
}

// ── Legacy stubs (removed in Stage 3) ────────────────────────────────────────
// `update_ewma`, `get_ewma`, `increment`, `decrement`, `get_count` are gone.
// Call `EdgionBackend::{update_ewma, inc_conn, dec_conn, ewma, conn}` directly.
