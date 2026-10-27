//! Eagerly compiled selectors owned by one Service selector generation.
//!
//! Backends are stored as `Arc<LbBackend>` so selectors return the live
//! object (with its metric atomics). `RrCore` / `ChCore` resolve
//! `pingora::Backend` → `Arc<LbBackend>` via an addr-keyed map built at
//! compile time, with no per-request allocation. `MetricCore` (LeastConn /
//! EWMA) drives P2C over per-backend atomic counters.

use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
use pingora_load_balancing::Backend;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::lb::backend::LbBackend;
use crate::lb::selection::p2c::select_p2c;
use crate::lb::selection::{consistent_hash::ConsistentHashRing, round_robin::RoundRobinSelector};

/// LB policy discriminator used to pick one eager selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyKind {
    RoundRobin,
    ConsistentHash,
    LeastConn,
    Ewma,
}

/// Maximum number of fallback iterations when a healthy backend is rejected.
const SELECT_MAX_ITERATIONS: usize = 256;

/// Build a map from `std::net::SocketAddr` → `Arc<LbBackend>` for fast
/// resolution after the inner pingora selector picks a `Backend` by address.
fn build_addr_map(backends: &[Arc<LbBackend>]) -> HashMap<SocketAddr, Arc<LbBackend>> {
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
/// `Arc<LbBackend>` via the addr-keyed map. Non-Inet addresses (which the
/// addr_map never contains) resolve to `None`. Does not re-apply the health
/// filter: the chosen backend already passed it during the selector walk, so
/// re-checking here would only open a TOCTOU window that could drop a valid pick.
fn resolve_backend<'a>(b: &Backend, addr_map: &'a HashMap<SocketAddr, Arc<LbBackend>>) -> Option<&'a Arc<LbBackend>> {
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
    addr_map: &HashMap<SocketAddr, Arc<LbBackend>>,
    health_filter: &impl Fn(&LbBackend) -> bool,
) -> bool {
    resolve_backend(b, addr_map).is_some_and(|eb| health_filter(eb))
}

/// Round-robin core: holds the canonical `Arc<LbBackend>` list plus the
/// pingora-backed weighted selector. The selector is materialized once at
/// compile time from the LbBackend slice. A per-addr map resolves the
/// `Backend` returned by the inner selector back to the live `Arc<LbBackend>`
/// so no rewrapping occurs on the hot path.
pub struct RrCore {
    backends: Arc<[Arc<LbBackend>]>,
    selector: RoundRobinSelector,
    addr_map: HashMap<SocketAddr, Arc<LbBackend>>,
}

impl RrCore {
    fn build(backends: Arc<[Arc<LbBackend>]>) -> Self {
        let pingora_view: Vec<Backend> = backends.iter().map(|b| b.inner().clone()).collect();
        let selector = RoundRobinSelector::build(&pingora_view);
        let addr_map = build_addr_map(&backends);
        Self {
            backends,
            selector,
            addr_map,
        }
    }

    fn select(&self, health_filter: impl Fn(&LbBackend) -> bool) -> Option<Arc<LbBackend>> {
        if self.backends.is_empty() {
            return None;
        }
        let addr_map = &self.addr_map;
        self.selector
            .select(SELECT_MAX_ITERATIONS, |b| {
                backend_passes_health(b, addr_map, &health_filter)
            })
            .and_then(|b| resolve_backend(b, addr_map).cloned())
    }
}

/// Consistent-hash core: same idea as `RrCore`. The inner ring still uses
/// pingora `Backend` internally.
///
/// Holds an internal `RrCore` for the empty-hash-key fallback so we don't
/// allocate a fresh `Vec<Backend>` and reset RR fairness on every keyless
/// request (real-world scenario: missing cookie / unauthenticated traffic).
pub struct ChCore {
    backends: Arc<[Arc<LbBackend>]>,
    ring: ConsistentHashRing,
    rr_fallback: RrCore,
}

impl ChCore {
    fn build(backends: Arc<[Arc<LbBackend>]>) -> Self {
        let pingora_view: Vec<Backend> = backends.iter().map(|b| b.inner().clone()).collect();
        let ring = ConsistentHashRing::build(&pingora_view);
        let rr_fallback = RrCore::build(backends.clone());
        Self {
            backends,
            ring,
            rr_fallback,
        }
    }

    fn select(&self, hash_key: &[u8], health_filter: impl Fn(&LbBackend) -> bool) -> Option<Arc<LbBackend>> {
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
            .and_then(|b| resolve_backend(b, addr_map).cloned())
    }
}

/// Metric-driven core (LeastConn / EWMA). Holds the canonical
/// `Arc<LbBackend>` list; selection is done via P2C over per-backend
/// atomic counters.
pub struct MetricCore {
    backends: Arc<[Arc<LbBackend>]>,
}

impl MetricCore {
    fn build(backends: Arc<[Arc<LbBackend>]>) -> Self {
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
    /// Borrow this selector's own backend list. Used by the degrade-threshold
    /// counting pass so the count and the selection walk operate on the exact
    /// same list (no `replace_backends` TOCTOU skew).
    fn backends_slice(&self) -> &[Arc<LbBackend>] {
        match self {
            CompiledSelector::RoundRobin(c) => &c.backends,
            CompiledSelector::ConsistentHash(c) => &c.backends,
            CompiledSelector::LeastConn(c) => &c.backends,
            CompiledSelector::Ewma(c) => &c.backends,
        }
    }

    /// Decide whether degrade mode should engage: count how many backends pass
    /// the (side-effect-free) `quiet` predicate, returning `true` when fewer
    /// than `floor` survive. Early-exits as soon as `floor` survivors are seen,
    /// so the common healthy case scans at most `floor` backends. `floor == 0`
    /// disables degrade mode (returns `false`).
    pub fn degrade_should_engage(&self, floor: usize, quiet: impl Fn(&LbBackend) -> bool) -> bool {
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

    /// Select a backend, returning a shared `Arc<LbBackend>` so callers
    /// can call metric methods on the live object without taking ownership.
    ///
    /// * `hash_key` — `Some` only for ConsistentHash; an empty key falls back
    ///   to round-robin to preserve the historical behaviour of the old
    ///   `select_from_backends`.
    /// * `health_filter` — combined active-health-check + circuit-breaker
    ///   predicate, supplied by the caller.
    /// * `now_units` — decay clock for the Ewma policy. Callers read it fresh
    ///   (`crate::lb::ewma::score::now_units`). `upstream.start_time` is
    ///   per-attempt and would also be correct, but is not plumbed here;
    ///   request-level start/test timestamps must never be used — they
    ///   are request-scoped and stale by multiple half-lives on a retried or
    ///   long-lived streaming request. Taking it as a parameter keeps the decay
    ///   clock injectable in tests.
    pub fn select(
        &self,
        hash_key: Option<&[u8]>,
        health_filter: impl Fn(&LbBackend) -> bool,
        now_units: u32,
    ) -> Option<Arc<LbBackend>> {
        match self {
            CompiledSelector::RoundRobin(core) => core.select(health_filter),
            CompiledSelector::ConsistentHash(core) => {
                // Empty hash key → fall back to round-robin (e.g. missing
                // cookie / unauthenticated traffic). Reuses the pre-built
                // `rr_fallback` to avoid per-pick allocation.
                match hash_key {
                    Some(k) if !k.is_empty() => core.select(k, health_filter),
                    _ => core.rr_fallback.select(health_filter),
                }
            }

            // P2C over per-backend conn_count atomics.
            CompiledSelector::LeastConn(core) => {
                select_p2c(&core.backends, |b| b.conn(), health_filter, SELECT_MAX_ITERATIONS)
            }
            // P2C over the decayed Peak-EWMA score times outstanding load.
            CompiledSelector::Ewma(core) => select_p2c(
                &core.backends,
                |b| b.score(now_units),
                health_filter,
                SELECT_MAX_ITERATIONS,
            ),
        }
    }
}

pub(crate) fn compile(kind: PolicyKind, backends: Arc<[Arc<LbBackend>]>) -> CompiledSelector {
    match kind {
        PolicyKind::RoundRobin => CompiledSelector::RoundRobin(RrCore::build(backends)),
        PolicyKind::ConsistentHash => CompiledSelector::ConsistentHash(ChCore::build(backends)),
        PolicyKind::LeastConn => CompiledSelector::LeastConn(MetricCore::build(backends)),
        PolicyKind::Ewma => CompiledSelector::Ewma(MetricCore::build(backends)),
    }
}

/// All load-balancing policies compiled from one immutable backend snapshot.
///
/// Construction is intentionally cold-path only. Request handling selects one
/// of these already-built values and never allocates a selector or takes a
/// build lock.
pub struct CompiledSelectors {
    pub round_robin: Arc<CompiledSelector>,
    pub consistent_hash: Arc<CompiledSelector>,
    pub least_conn: Arc<CompiledSelector>,
    pub ewma: Arc<CompiledSelector>,
}

#[cfg(test)]
pub(crate) use tests::selector_set_build_count_for_test;

impl CompiledSelectors {
    pub(crate) fn build(backends: Arc<[Arc<LbBackend>]>) -> Self {
        #[cfg(test)]
        tests::record_selector_set_build();
        Self {
            round_robin: Arc::new(compile(PolicyKind::RoundRobin, backends.clone())),
            consistent_hash: Arc::new(compile(PolicyKind::ConsistentHash, backends.clone())),
            least_conn: Arc::new(compile(PolicyKind::LeastConn, backends.clone())),
            ewma: Arc::new(compile(PolicyKind::Ewma, backends)),
        }
    }

    #[inline]
    pub fn get(&self, kind: PolicyKind) -> &Arc<CompiledSelector> {
        match kind {
            PolicyKind::RoundRobin => &self.round_robin,
            PolicyKind::ConsistentHash => &self.consistent_hash,
            PolicyKind::LeastConn => &self.least_conn,
            PolicyKind::Ewma => &self.ewma,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/lb/compiled_selector_test/degrade_should_engage_tests.rs"]
mod degrade_should_engage_tests;

#[cfg(test)]
#[path = "../../tests/unit/lb/compiled_selector_test.rs"]
mod tests;
