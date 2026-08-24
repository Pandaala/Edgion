//! Service-scoped LB runtime state — SP affinity + backend lifecycle ledger.
//!
//! Two responsibilities:
//!   1. Session-persistence affinity map (token → endpoint SocketAddr).
//!   2. Backend lifecycle ledger: draining registry.
//!
//! Metric atomics (`ewma_us`, `conn_count`) live on `EdgionBackend` itself.
//! `DRAINING_BACKENDS` holds the live `Arc<EdgionBackend>` so `BackendCleaner`
//! can poll `backend.conn()` without a side counter.

use dashmap::DashMap;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::sync::{Arc, LazyLock};

use super::edgion_backend::EdgionBackend;

// ── SP affinity map ───────────────────────────────────────────────────────────

/// Session affinity map: token (`generate_session_token` hex) → endpoint SocketAddr, per service key.
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

fn build_sp_affinity_map(
    backends: &[Arc<EdgionBackend>],
    scope: Option<&str>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
    for b in backends.iter().take(SP_AFFINITY_MAX_PER_SERVICE) {
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
    backends_fn: impl FnOnce() -> Arc<Vec<Arc<EdgionBackend>>>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    sp_affinity_get_or_build_scoped(service_key, None, backends_fn)
}

pub(crate) fn sp_affinity_get_or_build_scoped(
    service_key: &str,
    scope: Option<&str>,
    backends_fn: impl FnOnce() -> Arc<Vec<Arc<EdgionBackend>>>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    // Typed scopes may include a direct backend UID. Keeping those generations in
    // this process-global Service cache would let an in-flight request recreate
    // a retired scope after hot-reload cleanup. Build the bounded view from the
    // request-captured endpoint Arc instead; only ordinary Service affinity is
    // cached globally.
    if scope.is_some() {
        return build_sp_affinity_map(&backends_fn(), scope);
    }
    let cache_key = match scope {
        Some(_) => unreachable!(),
        None => service_key.to_string(),
    };
    SP_AFFINITY
        .entry(cache_key)
        .or_insert_with(|| build_sp_affinity_map(&backends_fn(), scope))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::http::session_persistence::{generate_scoped_session_token, generate_session_token};
    use pingora_load_balancing::Backend;
    use std::net::SocketAddr as StdSocketAddr;

    fn backend(addr: StdSocketAddr) -> Arc<EdgionBackend> {
        Arc::new(EdgionBackend::new(Backend {
            addr: SocketAddr::Inet(addr),
            weight: 1,
            ext: Default::default(),
        }))
    }

    /// Round-trip test: a session token derived from the backend cell address via
    /// `SocketAddr::to_string()` must resolve back through the affinity map for
    /// both IPv4 and IPv6 backends. Mis-formatted forms must NOT resolve.
    #[test]
    fn sp_affinity_round_trip_ipv4_and_ipv6() {
        let v4: StdSocketAddr = "10.0.0.1:8080".parse().unwrap();
        let v6: StdSocketAddr = "[::1]:30001".parse().unwrap();
        let svc = "ns/sp-round-trip";
        let backends = Arc::new(vec![backend(v4), backend(v6)]);
        let map = sp_affinity_get_or_build_key(svc, || backends.clone());
        let lookup = |token: &str| map.get(token).copied();

        // Token format: backend cell address formatted with `SocketAddr::to_string()`.
        let v4_token = generate_session_token(&v4.to_string());
        let v6_token = generate_session_token(&v6.to_string());
        assert_eq!(lookup(&v4_token), Some(v4));
        assert_eq!(lookup(&v6_token), Some(v6));

        // Unbracketed IPv6 form (`::1:30001`, from `format!("{}:{}", ip, port)`)
        // must NOT resolve — it is not a valid `SocketAddr::to_string()` output.
        let v6_unbracketed = generate_session_token(&format!("{}:{}", v6.ip(), v6.port()));
        assert_eq!(lookup(&v6_unbracketed), None);

        // Token built from a rewritten peer port must NOT resolve: session tokens
        // key off the cell address, not the post-rewritten peer address.
        let v6_rewritten_port = generate_session_token("[::1]:8080");
        assert_eq!(lookup(&v6_rewritten_port), None);

        remove_service(svc);
    }

    #[test]
    fn scoped_affinity_views_are_isolated_and_never_registered_globally() {
        let addr: StdSocketAddr = "10.0.0.8:8080".parse().unwrap();
        let svc = "ns/scoped-affinity";
        let backends = Arc::new(vec![backend(addr)]);
        let first_scope = "edgion.io/EdgionBackend/ns/first\0core/Service/ns/scoped-affinity";
        let second_scope = "edgion.io/EdgionBackend/ns/second\0core/Service/ns/scoped-affinity";
        let first = sp_affinity_get_or_build_scoped(svc, Some(first_scope), || backends.clone());
        let second = sp_affinity_get_or_build_scoped(svc, Some(second_scope), || backends.clone());

        let first_token = generate_scoped_session_token(Some(first_scope), &addr.to_string());
        let second_token = generate_scoped_session_token(Some(second_scope), &addr.to_string());
        assert_eq!(first.get(&first_token), Some(&addr));
        assert!(!first.contains_key(&second_token));
        assert_eq!(second.get(&second_token), Some(&addr));

        let rebuilt = sp_affinity_get_or_build_scoped(svc, Some(first_scope), || backends.clone());
        assert!(!Arc::ptr_eq(&first, &rebuilt));
        assert!(!SP_AFFINITY.contains_key(&format!("{svc}\0{first_scope}")));
        assert!(!SP_AFFINITY.contains_key(&format!("{svc}\0{second_scope}")));

        let ordinary_first = sp_affinity_get_or_build_key(svc, || backends.clone());
        let ordinary_second = sp_affinity_get_or_build_key(svc, || panic!("ordinary cache must be reused"));
        assert!(Arc::ptr_eq(&ordinary_first, &ordinary_second));
        remove_service(svc);
    }

    #[test]
    fn scoped_affinity_view_has_the_canonical_endpoint_cap() {
        let svc = "ns/scoped-bounded";
        let scope = "edgion.io/EdgionBackend/ns/agent/uid\0core/Service/ns/scoped-bounded";
        let backends = Arc::new(
            (0..=SP_AFFINITY_MAX_PER_SERVICE)
                .map(|index| {
                    let third = ((index / 254) % 254 + 1) as u8;
                    let fourth = (index % 254 + 1) as u8;
                    backend(StdSocketAddr::from(([10, 1, third, fourth], 8080)))
                })
                .collect(),
        );
        let view = sp_affinity_get_or_build_scoped(svc, Some(scope), || backends);
        assert_eq!(view.len(), SP_AFFINITY_MAX_PER_SERVICE);
        assert!(!SP_AFFINITY.contains_key(&format!("{svc}\0{scope}")));
    }
}
