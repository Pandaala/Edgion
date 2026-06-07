//! `CompiledSelector` + `ServiceSelectorCell` — Stage 3 of the lb-precompile
//! refactor.
//!
//! A `ServiceSelectorCell` holds the canonical `Arc<Vec<Arc<EdgionBackend>>>`
//! for a service, plus four lazily compiled per-policy selector slots stored
//! as `ArcSwapOption<CompiledSelector>`. The data plane fetches a precompiled
//! selector with a single atomic load on the hot path; the slow path goes
//! through a double-checked-lock build that consumes the current backend
//! snapshot.
//!
//! The control plane calls `replace_backends` whenever EP/EPS produces a new
//! backend list. That atomically swaps the snapshot and clears all four
//! policy slots so the next request rebuilds them from fresh data.
//!
//! Stage 3 changes:
//!   * Backends stored as `Arc<EdgionBackend>` so selectors return the live
//!     object (with its atomics) rather than a freshly-wrapped copy.
//!   * `RrCore` / `ChCore` resolve `pingora::Backend` → `Arc<EdgionBackend>`
//!     via an addr-keyed map built at compile time. No per-request allocation.
//!   * `MetricCore` (LeastConn / EWMA) now drives P2C over per-backend atomics
//!     instead of the legacy `runtime_state` DashMap lookups.

use arc_swap::{ArcSwap, ArcSwapOption};
use dashmap::DashMap;
use parking_lot::Mutex;
use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
use pingora_load_balancing::Backend;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};

use crate::core::gateway::lb::edgion_backend::EdgionBackend;
use crate::core::gateway::lb::selection::p2c::select_p2c;
use crate::core::gateway::lb::selection::{consistent_hash::ConsistentHashRing, round_robin::RoundRobinSelector};

/// LB policy discriminator used to pick the right slot in a
/// [`ServiceSelectorCell`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyKind {
    RoundRobin,
    ConsistentHash,
    LeastConn,
    Ewma,
}

/// Maximum number of fallback iterations when a healthy backend is rejected.
const SELECT_MAX_ITERATIONS: usize = 256;

/// Build a map from `std::net::SocketAddr` → `Arc<EdgionBackend>` for fast
/// resolution after the inner pingora selector picks a `Backend` by address.
fn build_addr_map(backends: &[Arc<EdgionBackend>]) -> HashMap<SocketAddr, Arc<EdgionBackend>> {
    backends
        .iter()
        .filter_map(|b| {
            if let PSocketAddr::Inet(addr) = b.addr {
                Some((addr, b.clone()))
            } else {
                None
            }
        })
        .collect()
}

/// Resolve the `pingora::Backend` chosen by an inner selector back to the live
/// `Arc<EdgionBackend>` via the addr-keyed map. Non-Inet addresses (which the
/// addr_map never contains) resolve to `None`. Does not re-apply the health
/// filter: the chosen backend already passed it during the selector walk, so
/// re-checking here would only open a TOCTOU window that could drop a valid pick.
fn resolve_backend<'a>(
    b: &Backend,
    addr_map: &'a HashMap<SocketAddr, Arc<EdgionBackend>>,
) -> Option<&'a Arc<EdgionBackend>> {
    if let PSocketAddr::Inet(addr) = b.addr {
        addr_map.get(&addr)
    } else {
        None
    }
}

/// Candidate predicate for an inner selector: resolve the `Backend` through the
/// addr-keyed map and apply the caller's combined active-health-check +
/// circuit-breaker filter. Shared by both `RrCore` and `ChCore` so the two
/// selection paths cannot drift apart.
fn backend_passes_health(
    b: &Backend,
    addr_map: &HashMap<SocketAddr, Arc<EdgionBackend>>,
    health_filter: &impl Fn(&EdgionBackend) -> bool,
) -> bool {
    resolve_backend(b, addr_map).is_some_and(|eb| health_filter(eb))
}

/// Round-robin core: holds the canonical `Arc<EdgionBackend>` list plus the
/// pingora-backed weighted selector. The selector is materialized once at
/// compile time from the EdgionBackend slice. A per-addr map resolves the
/// `Backend` returned by the inner selector back to the live `Arc<EdgionBackend>`
/// so no rewrapping occurs on the hot path.
pub struct RrCore {
    backends: Arc<Vec<Arc<EdgionBackend>>>,
    selector: RoundRobinSelector,
    addr_map: HashMap<SocketAddr, Arc<EdgionBackend>>,
}

impl RrCore {
    fn build(backends: Arc<Vec<Arc<EdgionBackend>>>) -> Self {
        let pingora_view: Vec<Backend> = backends.iter().map(|b| b.inner().clone()).collect();
        let selector = RoundRobinSelector::build(&pingora_view);
        let addr_map = build_addr_map(&backends);
        Self {
            backends,
            selector,
            addr_map,
        }
    }

    fn select(&self, health_filter: impl Fn(&EdgionBackend) -> bool) -> Option<Arc<EdgionBackend>> {
        if self.backends.is_empty() {
            return None;
        }
        let addr_map = &self.addr_map;
        self.selector
            .select(SELECT_MAX_ITERATIONS, |b| {
                backend_passes_health(b, addr_map, &health_filter)
            })
            .and_then(|b| resolve_backend(&b, addr_map).cloned())
    }
}

/// Consistent-hash core: same idea as `RrCore`. The inner ring still uses
/// pingora `Backend` internally.
///
/// Holds an internal `RrCore` for the empty-hash-key fallback so we don't
/// allocate a fresh `Vec<Backend>` and reset RR fairness on every keyless
/// request (real-world scenario: missing cookie / unauthenticated traffic).
pub struct ChCore {
    backends: Arc<Vec<Arc<EdgionBackend>>>,
    ring: ConsistentHashRing,
    rr_fallback: RrCore,
}

impl ChCore {
    fn build(backends: Arc<Vec<Arc<EdgionBackend>>>) -> Self {
        let pingora_view: Vec<Backend> = backends.iter().map(|b| b.inner().clone()).collect();
        let ring = ConsistentHashRing::build(&pingora_view);
        let rr_fallback = RrCore::build(backends.clone());
        Self {
            backends,
            ring,
            rr_fallback,
        }
    }

    fn select(&self, hash_key: &[u8], health_filter: impl Fn(&EdgionBackend) -> bool) -> Option<Arc<EdgionBackend>> {
        if self.backends.is_empty() {
            return None;
        }
        // Reuse the addr_map owned by the fallback RrCore instead of holding a
        // second identical copy: both are built from the same backend slice.
        let addr_map = &self.rr_fallback.addr_map;
        self.ring
            .select(hash_key, SELECT_MAX_ITERATIONS, |b| {
                backend_passes_health(b, addr_map, &health_filter)
            })
            .and_then(|b| resolve_backend(&b, addr_map).cloned())
    }
}

/// Metric-driven core (LeastConn / EWMA). Holds the canonical
/// `Arc<EdgionBackend>` list; selection is done via P2C over per-backend
/// atomic counters.
pub struct MetricCore {
    backends: Arc<Vec<Arc<EdgionBackend>>>,
}

impl MetricCore {
    fn build(backends: Arc<Vec<Arc<EdgionBackend>>>) -> Self {
        Self { backends }
    }
}

/// A precompiled per-service selector for a given policy.
pub enum CompiledSelector {
    RoundRobin(RrCore),
    ConsistentHash(ChCore),
    LeastConn(MetricCore),
    Ewma(MetricCore),
}

impl CompiledSelector {
    /// Borrow this selector's own backend list. Used by the panic-threshold
    /// counting pass so the count and the selection walk operate on the exact
    /// same list (no `replace_backends` TOCTOU skew).
    fn backends_slice(&self) -> &[Arc<EdgionBackend>] {
        match self {
            CompiledSelector::RoundRobin(c) => &c.backends,
            CompiledSelector::ConsistentHash(c) => &c.backends,
            CompiledSelector::LeastConn(c) => &c.backends,
            CompiledSelector::Ewma(c) => &c.backends,
        }
    }

    /// Decide whether panic mode should engage: count how many backends pass
    /// the (side-effect-free) `quiet` predicate, returning `true` when fewer
    /// than `floor` survive. Early-exits as soon as `floor` survivors are seen,
    /// so the common healthy case scans at most `floor` backends. `floor == 0`
    /// disables panic (returns `false`).
    pub fn panic_should_engage(&self, floor: usize, quiet: impl Fn(&EdgionBackend) -> bool) -> bool {
        if floor == 0 {
            return false;
        }
        let mut survivors = 0usize;
        for b in self.backends_slice() {
            if quiet(b) {
                survivors += 1;
                if survivors >= floor {
                    return false;
                }
            }
        }
        true
    }

    /// Number of backends behind this selector. Useful for tests / metrics.
    pub fn backend_count(&self) -> usize {
        match self {
            CompiledSelector::RoundRobin(c) => c.backends.len(),
            CompiledSelector::ConsistentHash(c) => c.backends.len(),
            CompiledSelector::LeastConn(c) => c.backends.len(),
            CompiledSelector::Ewma(c) => c.backends.len(),
        }
    }

    /// Select a backend, returning a shared `Arc<EdgionBackend>` so callers
    /// can call metric methods on the live object without taking ownership.
    ///
    /// * `hash_key` — `Some` only for ConsistentHash; an empty key falls back
    ///   to round-robin to preserve the historical behaviour of the old
    ///   `select_from_backends`.
    /// * `health_filter` — combined active-health-check + circuit-breaker
    ///   predicate, supplied by the caller.
    pub fn select(
        &self,
        hash_key: Option<&[u8]>,
        health_filter: impl Fn(&EdgionBackend) -> bool,
    ) -> Option<Arc<EdgionBackend>> {
        match self {
            CompiledSelector::RoundRobin(core) => core.select(health_filter),
            CompiledSelector::ConsistentHash(core) => {
                // Empty hash key → behave as round-robin. Mirrors the old
                // `select_from_backends` branch where an empty key produced an
                // RR selection (with its own not-found error code mapped by
                // the caller). Reuses the pre-built `rr_fallback` so an
                // unauthenticated/keyless request does not allocate per pick.
                match hash_key {
                    Some(k) if !k.is_empty() => core.select(k, health_filter),
                    _ => core.rr_fallback.select(health_filter),
                }
            }

            // P2C over per-backend conn_count atomics.
            CompiledSelector::LeastConn(core) => {
                select_p2c(&core.backends, |b| b.conn(), health_filter, SELECT_MAX_ITERATIONS)
            }
            // P2C over per-backend ewma_us atomics.
            CompiledSelector::Ewma(core) => {
                select_p2c(&core.backends, |b| b.ewma(), health_filter, SELECT_MAX_ITERATIONS)
            }
        }
    }
}

/// Per-service container for the precompiled selectors of all four policies.
///
/// The control plane writes new backend lists via `replace_backends`. The data
/// plane reads through `get_or_build`, which lazily compiles the selector for
/// the requested policy under a double-checked lock.
pub struct ServiceSelectorCell {
    /// Current authoritative backend list (shared Arc per backend so selectors
    /// can return the live object with its atomics). Replaced atomically;
    /// readers under `build_lock` see whichever list was stored last before
    /// the build began.
    pub(crate) backends: ArcSwap<Vec<Arc<EdgionBackend>>>,
    rr: ArcSwapOption<CompiledSelector>,
    ch: ArcSwapOption<CompiledSelector>,
    least_conn: ArcSwapOption<CompiledSelector>,
    ewma: ArcSwapOption<CompiledSelector>,
    /// Single mutex covering the "compile a slot" critical section for all
    /// four policies. Only contended on first build per (service, policy).
    build_lock: Mutex<()>,
}

impl ServiceSelectorCell {
    pub fn new(initial: Vec<Arc<EdgionBackend>>) -> Self {
        Self {
            backends: ArcSwap::from_pointee(initial),
            rr: ArcSwapOption::const_empty(),
            ch: ArcSwapOption::const_empty(),
            least_conn: ArcSwapOption::const_empty(),
            ewma: ArcSwapOption::const_empty(),
            build_lock: Mutex::new(()),
        }
    }

    /// Atomically replace the backend snapshot and invalidate all precompiled
    /// selector slots. The next `get_or_build(kind)` call rebuilds the
    /// requested slot from the new snapshot.
    ///
    /// Backends whose address already exists in the current snapshot reuse the
    /// existing `Arc<EdgionBackend>` so accumulated EWMA and conn_count survive
    /// EP/EPS-driven updates (rolling deploys, readiness probe flaps, etc.).
    /// Only genuinely new pod IPs receive fresh wrapper objects.
    ///
    /// Both the backend store and the slot clears are performed under `build_lock`
    /// so that a concurrent `get_or_build` call always sees either the pre-replace
    /// state (old backends + old slots) or the post-replace state (new backends +
    /// empty slots). The intermediate state (new backends + old addr_map slot) would
    /// cause spurious upstream-not-found errors on RR/CH paths.
    pub fn replace_backends(&self, new: Vec<EdgionBackend>) {
        let _guard = self.build_lock.lock();
        let old_snap = self.backends.load();
        let old_map: HashMap<SocketAddr, Arc<EdgionBackend>> = old_snap
            .iter()
            .filter_map(|b| {
                if let PSocketAddr::Inet(addr) = b.addr {
                    Some((addr, b.clone()))
                } else {
                    None
                }
            })
            .collect();

        // NOTE: when an addr matches, the *new* EdgionBackend (with potentially
        // updated weight/ext) is discarded in favour of the old Arc. This is
        // intentional: today all callers hard-code weight=1 and ext is not used
        // by any selector, so silently dropping the new inner is safe. If
        // per-backend weights or ext-driven selection are ever introduced, this
        // carry-over must be updated to transfer the new inner onto the reused Arc
        // (or rebuild a fresh Arc when inner fields differ).
        let arced: Vec<Arc<EdgionBackend>> = new
            .into_iter()
            .map(|b| {
                if let PSocketAddr::Inet(addr) = b.addr {
                    old_map.get(&addr).cloned().unwrap_or_else(|| Arc::new(b))
                } else {
                    Arc::new(b)
                }
            })
            .collect();
        self.backends.store(Arc::new(arced));
        self.rr.store(None);
        self.ch.store(None);
        self.least_conn.store(None);
        self.ewma.store(None);
    }

    /// Seed the cell with backends from `provider` if the cell is currently empty.
    ///
    /// Uses a double-checked lock against `build_lock` — the same lock held by
    /// `replace_backends` — so the provider read and the subsequent `store` are
    /// atomic with respect to any concurrent control-plane `replace_backends` call.
    /// This eliminates the TOCTOU race where hydrate could overwrite a fresh
    /// control-plane update with stale data-layer content.
    ///
    /// The data-plane fast path (cell already non-empty) never acquires the lock.
    /// Lock contention only occurs on the very first request for a new service.
    ///
    /// Returns `true` if the cell is non-empty after this call.
    pub fn hydrate_if_empty(&self, provider: impl FnOnce() -> Vec<EdgionBackend>) -> bool {
        // Fast path: no lock needed.
        if !self.backends.load().is_empty() {
            return true;
        }
        // Slow path: double-checked lock shared with replace_backends.
        let _guard = self.build_lock.lock();
        if !self.backends.load().is_empty() {
            return true;
        }
        let backends = provider();
        if backends.is_empty() {
            return false;
        }
        // Cell is empty and we hold build_lock, so no metric carry-over is needed
        // (there are no old Arcs to preserve). Wrap directly.
        let arced: Vec<Arc<EdgionBackend>> = backends.into_iter().map(Arc::new).collect();
        self.backends.store(Arc::new(arced));
        true
    }

    /// Snapshot the current backend list (cheap Arc clone, no copy of backends).
    pub fn backends_snapshot(&self) -> Arc<Vec<Arc<EdgionBackend>>> {
        self.backends.load_full()
    }

    /// Get or lazily build the selector for the requested policy.
    pub fn get_or_build(&self, kind: PolicyKind) -> Arc<CompiledSelector> {
        let slot = self.slot_for(kind);

        if let Some(s) = slot.load_full() {
            return s;
        }

        let _guard = self.build_lock.lock();
        if let Some(s) = slot.load_full() {
            return s;
        }

        let backends = self.backends.load_full();
        let new_sel = compile(kind, backends);
        let arc = Arc::new(new_sel);
        slot.store(Some(arc.clone()));
        arc
    }

    fn slot_for(&self, kind: PolicyKind) -> &ArcSwapOption<CompiledSelector> {
        match kind {
            PolicyKind::RoundRobin => &self.rr,
            PolicyKind::ConsistentHash => &self.ch,
            PolicyKind::LeastConn => &self.least_conn,
            PolicyKind::Ewma => &self.ewma,
        }
    }
}

fn compile(kind: PolicyKind, backends: Arc<Vec<Arc<EdgionBackend>>>) -> CompiledSelector {
    match kind {
        PolicyKind::RoundRobin => CompiledSelector::RoundRobin(RrCore::build(backends)),
        PolicyKind::ConsistentHash => CompiledSelector::ConsistentHash(ChCore::build(backends)),
        PolicyKind::LeastConn => CompiledSelector::LeastConn(MetricCore::build(backends)),
        PolicyKind::Ewma => CompiledSelector::Ewma(MetricCore::build(backends)),
    }
}

// ── Global cache ─────────────────────────────────────────────────────────

static SELECTOR_CACHE: LazyLock<DashMap<String, Arc<ServiceSelectorCell>>> = LazyLock::new(DashMap::new);

/// Get or create a `ServiceSelectorCell` for `service_key`. Newly created
/// cells start with an empty backend list; the control plane is expected to
/// call `replace_backends` shortly thereafter. A request that hits an
/// uninitialized cell selects on an empty list and returns `None`, which the
/// caller maps to the appropriate `not_found` status.
pub fn get_or_create_cell(service_key: &str) -> Arc<ServiceSelectorCell> {
    if let Some(cell) = SELECTOR_CACHE.get(service_key) {
        return cell.value().clone();
    }
    SELECTOR_CACHE
        .entry(service_key.to_string())
        .or_insert_with(|| Arc::new(ServiceSelectorCell::new(Vec::<Arc<EdgionBackend>>::new())))
        .value()
        .clone()
}

/// Return all service keys currently tracked in the global cell cache.
///
/// Used by `full_set` handlers to determine which services were known before
/// the replacement, so stale cells (services deleted without HC annotations)
/// are correctly cleaned up regardless of whether they appear in the HC config store.
pub fn all_cell_keys() -> Vec<String> {
    SELECTOR_CACHE.iter().map(|e| e.key().clone()).collect()
}

/// Drop the cell for `service_key`. Called when a Service is deleted; in-flight
/// requests that already hold an `Arc<ServiceSelectorCell>` keep working until
/// they drop it.
pub fn drop_cell(service_key: &str) {
    SELECTOR_CACHE.remove(service_key);
}

/// Look up an existing cell without creating one. Used by tests / diagnostics.
pub fn try_get_cell(service_key: &str) -> Option<Arc<ServiceSelectorCell>> {
    SELECTOR_CACHE.get(service_key).map(|c| c.value().clone())
}

#[cfg(test)]
mod panic_should_engage_tests {
    use super::*;
    use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
    use pingora_load_balancing::Backend;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn backends(n: u8) -> Vec<Arc<EdgionBackend>> {
        (0..n)
            .map(|i| {
                Arc::new(EdgionBackend::new(Backend {
                    addr: PSocketAddr::Inet(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)), 80)),
                    weight: 1,
                    ext: Default::default(),
                }))
            })
            .collect()
    }

    fn rr_selector(n: u8) -> CompiledSelector {
        CompiledSelector::RoundRobin(RrCore::build(Arc::new(backends(n))))
    }

    #[test]
    fn engages_when_survivors_below_floor() {
        let sel = rr_selector(10);
        // floor for 50% of 10 = 5; only 4 healthy → panic.
        // Cell<u32> allows interior mutation through an Fn closure.
        let healthy = std::cell::Cell::new(0u32);
        let engaged = sel.panic_should_engage(5, |_| {
            let h = healthy.get() + 1;
            healthy.set(h);
            h <= 4
        });
        assert!(engaged);
    }

    #[test]
    fn does_not_engage_when_survivors_reach_floor() {
        let sel = rr_selector(10);
        // 6 healthy >= floor 5 → no panic; early-exit after the 5th survivor.
        let engaged = sel.panic_should_engage(5, |_| true);
        assert!(!engaged);
    }

    #[test]
    fn floor_zero_never_engages() {
        let sel = rr_selector(3);
        assert!(!sel.panic_should_engage(0, |_| false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn backend(i: u32) -> EdgionBackend {
        let octets = i.to_be_bytes();
        let ip = IpAddr::V4(Ipv4Addr::new(10, octets[1], octets[2], octets[3]));
        EdgionBackend::new(Backend {
            addr: PSocketAddr::Inet(SocketAddr::new(ip, 80)),
            weight: 1,
            ext: Default::default(),
        })
    }

    fn arc_backend(i: u32) -> Arc<EdgionBackend> {
        Arc::new(backend(i))
    }

    #[test]
    fn replace_backends_clears_all_slots() {
        let cell = ServiceSelectorCell::new(vec![arc_backend(1), arc_backend(2)]);
        let _ = cell.get_or_build(PolicyKind::RoundRobin);
        let _ = cell.get_or_build(PolicyKind::LeastConn);
        assert!(cell.rr.load_full().is_some());
        assert!(cell.least_conn.load_full().is_some());

        cell.replace_backends(vec![backend(3)]);
        assert!(cell.rr.load_full().is_none());
        assert!(cell.ch.load_full().is_none());
        assert!(cell.least_conn.load_full().is_none());
        assert!(cell.ewma.load_full().is_none());
    }

    #[test]
    fn get_or_build_is_cached() {
        let cell = ServiceSelectorCell::new(vec![arc_backend(1)]);
        let a = cell.get_or_build(PolicyKind::RoundRobin);
        let b = cell.get_or_build(PolicyKind::RoundRobin);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn empty_cell_select_returns_none() {
        let cell = ServiceSelectorCell::new(Vec::new());
        let sel = cell.get_or_build(PolicyKind::RoundRobin);
        assert!(sel.select(None, |_| true).is_none());
    }

    #[test]
    fn rr_select_picks_a_backend() {
        let cell = ServiceSelectorCell::new(vec![arc_backend(1), arc_backend(2)]);
        let sel = cell.get_or_build(PolicyKind::RoundRobin);
        let picked = sel.select(None, |_| true);
        assert!(picked.is_some());
    }

    #[test]
    fn ch_with_empty_key_falls_back_to_rr() {
        let cell = ServiceSelectorCell::new(vec![arc_backend(1), arc_backend(2)]);
        let sel = cell.get_or_build(PolicyKind::ConsistentHash);
        let picked = sel.select(Some(b""), |_| true);
        assert!(picked.is_some());
    }

    #[test]
    fn lc_select_picks_lowest_conn_backend() {
        let a = arc_backend(1);
        let b = arc_backend(2);
        a.inc_conn();
        a.inc_conn(); // a has 2, b has 0
        let cell = ServiceSelectorCell::new(vec![a.clone(), b.clone()]);
        let sel = cell.get_or_build(PolicyKind::LeastConn);
        // P2C with equal metric would also pick b due to lower conn count
        for _ in 0..20 {
            let picked = sel.select(None, |_| true).expect("must pick");
            // b should dominate (conn=0 wins over conn=2 in P2C)
            assert!(
                Arc::ptr_eq(&picked, &a) || Arc::ptr_eq(&picked, &b),
                "must be one of our backends"
            );
        }
    }

    #[test]
    fn rr_select_returns_live_arc() {
        let b = arc_backend(1);
        let cell = ServiceSelectorCell::new(vec![b.clone()]);
        let sel = cell.get_or_build(PolicyKind::RoundRobin);
        let picked = sel.select(None, |_| true).expect("must pick");
        assert!(Arc::ptr_eq(&picked, &b), "must return the original Arc, not a rewrap");
    }

    #[test]
    fn replace_backends_carries_over_metrics_for_existing_addr() {
        let a = arc_backend(1);
        let b = arc_backend(2);
        // Simulate accumulated metric state on backend `a`.
        a.inc_conn();
        a.inc_conn();
        a.update_ewma(5_000);
        let cell = ServiceSelectorCell::new(vec![a.clone(), b.clone()]);

        // Replace with a list that still contains backend 1 (same addr) plus a new one.
        cell.replace_backends(vec![backend(1), backend(3)]);

        let snap = cell.backends_snapshot();
        assert_eq!(snap.len(), 2);

        // The backend with the same addr as `a` must be the original Arc.
        let carried = snap.iter().find(|eb| Arc::ptr_eq(eb, &a));
        assert!(carried.is_some(), "Arc for existing addr must be reused");

        let carried = carried.unwrap();
        assert_eq!(carried.conn(), 2, "conn_count must survive replace_backends");
        assert!(
            carried.ewma() > crate::core::gateway::lb::edgion_backend::INITIAL_EWMA_US,
            "ewma must survive replace_backends"
        );

        // Backend 3 is new — must be a fresh Arc (not equal to a or b).
        let fresh = snap.iter().find(|eb| !Arc::ptr_eq(eb, &a) && !Arc::ptr_eq(eb, &b));
        assert!(fresh.is_some(), "new addr must get a fresh Arc");
    }

    #[test]
    fn drop_cell_removes_entry() {
        let key = "test-ns/drop-cell-removes-entry";
        let _ = get_or_create_cell(key);
        assert!(try_get_cell(key).is_some());
        drop_cell(key);
        assert!(try_get_cell(key).is_none());
    }
}
