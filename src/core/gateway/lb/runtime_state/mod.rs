//! Service-scoped LB runtime state
//!
//! Replaces the global `SocketAddr`-keyed EWMA / LeastConn runtime state with
//! `service_key -> backend_addr -> metric` so that:
//!   - stale entries are cleaned with Service / Endpoint lifecycle
//!   - same SocketAddr reused by different services cannot share state
//!
//! Also caches `RoundRobinSelector` and `ConsistentHashRing` per service so
//! the atomic RR counter persists across requests and the ketama ring is not
//! rebuilt on every request.
//!
//! The outer key is `"namespace/name"` (same as route / discovery key).

use dashmap::DashMap;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::Backend;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

use super::leastconn::BackendState;
use super::selection::consistent_hash::ConsistentHashRing;
use super::selection::round_robin::RoundRobinSelector;

/// Per-service runtime metrics for LB algorithms.
pub struct ServiceRuntimeState {
    pub ewma: DashMap<SocketAddr, AtomicU64>,
    pub conn_counts: DashMap<SocketAddr, AtomicUsize>,
    pub backend_states: DashMap<SocketAddr, BackendState>,
}

impl ServiceRuntimeState {
    fn new() -> Self {
        Self {
            ewma: DashMap::new(),
            conn_counts: DashMap::new(),
            backend_states: DashMap::new(),
        }
    }
}

/// Global service-scoped runtime state.
/// Key: `"namespace/name"`, e.g. `"default/my-svc"`.
static SERVICES: LazyLock<DashMap<String, Arc<ServiceRuntimeState>>> = LazyLock::new(DashMap::new);

/// Cached `RoundRobinSelector` per service key.
static RR_CACHE: LazyLock<DashMap<String, Arc<RoundRobinSelector>>> = LazyLock::new(DashMap::new);

/// Cached `ConsistentHashRing` per service key.
static CH_CACHE: LazyLock<DashMap<String, Arc<RwLock<ConsistentHashRing>>>> = LazyLock::new(DashMap::new);

// ── Selector cache API ────────────────────────────────────────────────

/// Get or build a `RoundRobinSelector` for the service.
/// The selector is cached; call `invalidate_selector_cache` when backends change.
pub fn get_rr_selector(service_key: &str, backends: &[Backend]) -> Arc<RoundRobinSelector> {
    RR_CACHE
        .entry(service_key.to_string())
        .or_insert_with(|| Arc::new(RoundRobinSelector::build(backends)))
        .value()
        .clone()
}

/// Get or build a `ConsistentHashRing` for the service.
/// The ring is cached; call `invalidate_selector_cache` when backends change.
pub fn get_ch_ring(service_key: &str, backends: &[Backend]) -> Arc<RwLock<ConsistentHashRing>> {
    CH_CACHE
        .entry(service_key.to_string())
        .or_insert_with(|| Arc::new(RwLock::new(ConsistentHashRing::build(backends))))
        .value()
        .clone()
}

/// Invalidate cached selectors for a service (called when backends change).
pub fn invalidate_selector_cache(service_key: &str) {
    RR_CACHE.remove(service_key);
    CH_CACHE.remove(service_key);
}

// ── helpers ───────────────────────────────────────────────────────────

#[inline]
fn get_or_create_service(service_key: &str) -> Arc<ServiceRuntimeState> {
    if let Some(s) = SERVICES.get(service_key) {
        return s.value().clone();
    }
    SERVICES
        .entry(service_key.to_string())
        .or_insert_with(|| Arc::new(ServiceRuntimeState::new()))
        .value()
        .clone()
}

#[inline]
fn get_service(service_key: &str) -> Option<Arc<ServiceRuntimeState>> {
    SERVICES.get(service_key).map(|s| s.value().clone())
}

// ── EWMA API ──────────────────────────────────────────────────────────

/// Initial EWMA value for new backends (1 ms in µs).
const INITIAL_EWMA_US: u64 = 1_000;

/// Update EWMA for `(service_key, addr)`.
pub fn update_ewma(service_key: &str, addr: &SocketAddr, latency_us: u64) {
    let alpha = super::ewma::get_alpha();
    let svc = get_or_create_service(service_key);
    let entry = svc
        .ewma
        .entry(addr.clone())
        .or_insert_with(|| AtomicU64::new(INITIAL_EWMA_US));

    let mut current = entry.load(Ordering::Relaxed);
    loop {
        let new_ewma = (alpha as u64 * latency_us + (100 - alpha as u64) * current) / 100;
        match entry.compare_exchange_weak(current, new_ewma, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

/// Get EWMA for `(service_key, addr)`.
#[inline]
pub fn get_ewma(service_key: &str, addr: &SocketAddr) -> u64 {
    get_service(service_key)
        .and_then(|s| s.ewma.get(addr).map(|v| v.load(Ordering::Relaxed)))
        .unwrap_or(INITIAL_EWMA_US)
}

// ── Connection count API ──────────────────────────────────────────────

/// Increment connection count for `(service_key, addr)`.
pub fn increment(service_key: &str, addr: &SocketAddr) {
    let svc = get_or_create_service(service_key);
    svc.conn_counts
        .entry(addr.clone())
        .or_insert_with(|| AtomicUsize::new(0))
        .fetch_add(1, Ordering::Relaxed);
}

/// Decrement connection count for `(service_key, addr)`.
/// Uses CAS loop to avoid underflow from concurrent decrements.
pub fn decrement(service_key: &str, addr: &SocketAddr) {
    if let Some(svc) = get_service(service_key) {
        if let Some(count) = svc.conn_counts.get(addr) {
            let mut current = count.load(Ordering::Relaxed);
            loop {
                if current == 0 {
                    break;
                }
                match count.compare_exchange_weak(current, current - 1, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(actual) => current = actual,
                }
            }
        }
    }
}

/// Get connection count for `(service_key, addr)`.
#[inline]
pub fn get_count(service_key: &str, addr: &SocketAddr) -> usize {
    get_service(service_key)
        .and_then(|s| s.conn_counts.get(addr).map(|c| c.load(Ordering::Relaxed)))
        .unwrap_or(0)
}

// ── Backend state API ─────────────────────────────────────────────────

/// Set backend lifecycle state.
pub fn set_backend_state(service_key: &str, addr: &SocketAddr, state: BackendState) {
    let svc = get_or_create_service(service_key);
    svc.backend_states.insert(addr.clone(), state);
}

/// Get backend lifecycle state (defaults to `Active`).
#[inline]
pub fn get_backend_state(service_key: &str, addr: &SocketAddr) -> BackendState {
    get_service(service_key)
        .and_then(|s| s.backend_states.get(addr).map(|v| *v))
        .unwrap_or(BackendState::Active)
}

/// Check if backend is active.
#[inline]
pub fn is_backend_active(service_key: &str, addr: &SocketAddr) -> bool {
    matches!(get_backend_state(service_key, addr), BackendState::Active)
}

/// Mark backend as draining.
pub fn mark_backend_draining(service_key: &str, addr: &SocketAddr) {
    set_backend_state(service_key, addr, BackendState::Draining);
    tracing::info!(
        service_key = %service_key,
        backend = %addr,
        "Backend marked as draining"
    );
}

/// Reactivate a draining backend.
pub fn reactivate_backend(service_key: &str, addr: &SocketAddr) {
    set_backend_state(service_key, addr, BackendState::Active);
    tracing::info!(
        service_key = %service_key,
        backend = %addr,
        "Backend reactivated"
    );
}

// ── Cleanup API ───────────────────────────────────────────────────────

/// Remove all runtime state for a single backend under a service.
pub fn remove_backend(service_key: &str, addr: &SocketAddr) {
    if let Some(svc) = get_service(service_key) {
        svc.ewma.remove(addr);
        svc.conn_counts.remove(addr);
        svc.backend_states.remove(addr);
    }
}

/// Remove runtime state for multiple backends under a service.
pub fn remove_backends(service_key: &str, addrs: &[SocketAddr]) {
    if let Some(svc) = get_service(service_key) {
        for addr in addrs {
            svc.ewma.remove(addr);
            svc.conn_counts.remove(addr);
            svc.backend_states.remove(addr);
        }
    }
}

/// Remove the entire service runtime state and selector caches.
pub fn remove_service(service_key: &str) {
    SERVICES.remove(service_key);
    invalidate_selector_cache(service_key);
    tracing::info!(
        service_key = %service_key,
        "Removed service runtime state"
    );
}

/// Get all backends in draining state for a service.
pub fn get_draining_backends(service_key: &str) -> Vec<SocketAddr> {
    get_service(service_key)
        .map(|svc| {
            svc.backend_states
                .iter()
                .filter(|e| *e.value() == BackendState::Draining)
                .map(|e| e.key().clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Return all tracked service keys.
pub fn all_service_keys() -> Vec<String> {
    SERVICES.iter().map(|e| e.key().clone()).collect()
}
