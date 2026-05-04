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
use parking_lot::RwLock;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::Backend;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

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

/// Session affinity map: token (CRC32 hex) → endpoint SocketAddr, per service key.
/// Built lazily on first session persistence request. Invalidated when backends change.
/// Cap: max 1024 entries per service to prevent memory explosion.
static SP_AFFINITY: LazyLock<DashMap<String, Arc<std::collections::HashMap<String, std::net::SocketAddr>>>> =
    LazyLock::new(DashMap::new);

/// Max entries per service in the session affinity map.
const SP_AFFINITY_MAX_PER_SERVICE: usize = 1024;

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
    SP_AFFINITY.remove(service_key);
}

// ── Session Persistence Affinity Map ─────────────────────────────────

/// Look up a session token in the affinity map for a service.
/// Returns the endpoint SocketAddr if the token is known.
pub fn sp_affinity_lookup(service_key: &str, token: &str) -> Option<std::net::SocketAddr> {
    SP_AFFINITY.get(service_key).and_then(|map| map.get(token).copied())
}

/// Build or refresh the session affinity map for a service from its current backends.
/// Each backend's `ip:port` is hashed via CRC32 to produce the token key.
/// Capped at `SP_AFFINITY_MAX_PER_SERVICE` entries.
pub fn sp_affinity_build(service_key: &str, backends: &[Backend]) {
    let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
    for b in backends.iter().take(SP_AFFINITY_MAX_PER_SERVICE) {
        if let SocketAddr::Inet(addr) = b.addr {
            let addr_str = addr.to_string();
            let token = format!("{:08x}", crc32fast::hash(addr_str.as_bytes()));
            map.insert(token, addr);
        }
    }
    SP_AFFINITY.insert(service_key.to_string(), Arc::new(map));
}

/// Get or build the session affinity map for a service.
/// Lazy: builds from backends on first access.
pub fn sp_affinity_get_or_build(
    id: &crate::types::ServiceIdentity,
    backends: &[Backend],
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    sp_affinity_get_or_build_key(id.key(), backends)
}

/// Internal `&str` variant of [`sp_affinity_get_or_build`].
pub(crate) fn sp_affinity_get_or_build_key(
    service_key: &str,
    backends: &[Backend],
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    SP_AFFINITY
        .entry(service_key.to_string())
        .or_insert_with(|| {
            let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
            for b in backends.iter().take(SP_AFFINITY_MAX_PER_SERVICE) {
                if let SocketAddr::Inet(addr) = b.addr {
                    let addr_str = addr.to_string();
                    let token = format!("{:08x}", crc32fast::hash(addr_str.as_bytes()));
                    map.insert(token, addr);
                }
            }
            Arc::new(map)
        })
        .value()
        .clone()
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

#[cfg(test)]
mod tests {
    use super::*;
    use pingora_core::protocols::l4::socket::SocketAddr;

    fn inet(addr: &str) -> SocketAddr {
        SocketAddr::Inet(addr.parse().unwrap())
    }

    /// P0-07 回归测试：retry 路径下所有 backend 计数最终归零。
    ///
    /// 场景：连接 A → 出错 retry → 连接 B → 成功
    /// 期望：请求结束后 count(A) == 0, count(B) == 0
    #[test]
    fn test_leastconn_retry_counter_reaches_zero() {
        let svc = "test-ns/p0-07-retry-leak";
        let a = inet("127.0.0.1:19001");
        let b = inet("127.0.0.1:19002");

        // 模拟 connected_to_upstream(A): increment(A)
        increment(svc, &a);
        assert_eq!(get_count(svc, &a), 1, "A should be 1 after first connection");

        // 模拟 begin_upstream_attempt on retry: take+decrement(A) before push placeholder for B
        decrement(svc, &a);
        assert_eq!(get_count(svc, &a), 0, "A should reach 0 after retry cleanup");

        // 模拟 connected_to_upstream(B): increment(B)
        increment(svc, &b);
        assert_eq!(get_count(svc, &b), 1, "B should be 1 after second connection");

        // 模拟 logging: decrement(B) for final upstream
        decrement(svc, &b);
        assert_eq!(get_count(svc, &b), 0, "B should reach 0 after logging");

        // cleanup
        remove_service(svc);
    }

    /// 多次 retry 场景：A → B → C，全部归零。
    #[test]
    fn test_leastconn_multiple_retries_counter_reaches_zero() {
        let svc = "test-ns/p0-07-multi-retry";
        let a = inet("127.0.0.1:19011");
        let b = inet("127.0.0.1:19012");
        let c = inet("127.0.0.1:19013");

        // Attempt 1: connect A
        increment(svc, &a);
        // Retry 1: begin_upstream_attempt cleans A, then connects B
        decrement(svc, &a);
        increment(svc, &b);
        // Retry 2: begin_upstream_attempt cleans B, then connects C
        decrement(svc, &b);
        increment(svc, &c);
        // logging: decrement C
        decrement(svc, &c);

        assert_eq!(get_count(svc, &a), 0, "A count should be 0");
        assert_eq!(get_count(svc, &b), 0, "B count should be 0");
        assert_eq!(get_count(svc, &c), 0, "C count should be 0");

        remove_service(svc);
    }

    /// 连接失败（未进 connected_to_upstream）不产生 increment，decrement 是 no-op。
    #[test]
    fn test_leastconn_connect_failure_no_increment() {
        let svc = "test-ns/p0-07-connect-fail";
        let a = inet("127.0.0.1:19021");

        // 连接失败：从未调 increment
        assert_eq!(get_count(svc, &a), 0, "No increment means count stays 0");

        // decrement 对不存在的条目是 no-op（不 panic，不下溢）
        decrement(svc, &a);
        assert_eq!(get_count(svc, &a), 0, "Spurious decrement should be no-op");
        remove_service(svc);
    }
}
