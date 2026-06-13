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
            // Threat model: this CRC32 of `IP:port` (no secret, no MAC) is a
            // stickiness hint, NOT a security boundary. A client that knows a
            // backend's `IP:port` can forge the matching token to pin its own
            // requests to that backend, but the map only ever holds the service's
            // real backends, so a forged token can only pick an existing backend
            // (never an arbitrary address) and only affects the forging client's
            // own traffic. See `session_persistence::generate_session_token` for
            // the full rationale. Use an HMAC over a secret if affinity is ever
            // promoted to a routing restriction.
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
/// backend whose `conn()` reaches zero. The orphan is bounded by connection
/// lifetime, not a permanent residue. See
/// `docs/superpowers/specs/2026-06-09-gateway-cross-kind-derived-state-race-audit-design.md`
/// for the full audit.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::http::session_persistence::generate_session_token;
    use pingora_load_balancing::Backend;
    use std::net::SocketAddr as StdSocketAddr;

    fn backend(addr: StdSocketAddr) -> Arc<EdgionBackend> {
        Arc::new(EdgionBackend::new(Backend {
            addr: SocketAddr::Inet(addr),
            weight: 1,
            ext: Default::default(),
        }))
    }

    /// route-07 / route-08 regression: the response-side token (now derived from
    /// the LB cell address via `std::net::SocketAddr::to_string()`) must resolve
    /// back through the affinity map for both IPv4 and IPv6 backends, and the two
    /// historical mismatch forms must NOT resolve.
    #[test]
    fn sp_affinity_round_trip_ipv4_and_ipv6() {
        let v4: StdSocketAddr = "10.0.0.1:8080".parse().unwrap();
        let v6: StdSocketAddr = "[::1]:30001".parse().unwrap();
        let svc = "ns/sp-round-trip";
        sp_affinity_build(svc, &[backend(v4), backend(v6)]);

        // The token is generated exactly as the response filter now does it:
        // `lb_backend_addr` (the cell address) formatted with `SocketAddr::to_string()`.
        let v4_token = generate_session_token(&v4.to_string());
        let v6_token = generate_session_token(&v6.to_string());
        assert_eq!(sp_affinity_lookup(svc, &v4_token), Some(v4));
        assert_eq!(sp_affinity_lookup(svc, &v6_token), Some(v6));

        // route-07: the old unbracketed `format!("{}:{}", ip, port)` form
        // (`::1:30001`) must NOT resolve — it is the bug this fix removes.
        let v6_unbracketed = generate_session_token(&format!("{}:{}", v6.ip(), v6.port()));
        assert_eq!(sp_affinity_lookup(svc, &v6_unbracketed), None);

        // route-08: a token built from a rewritten peer port (cell port 30001 →
        // peer port 8080) must NOT resolve — the fix keys off the cell address,
        // not the post-`resolve_target_port_key` peer address.
        let v6_rewritten_port = generate_session_token("[::1]:8080");
        assert_eq!(sp_affinity_lookup(svc, &v6_rewritten_port), None);

        remove_service(svc);
    }
}
