//! Data-plane upstream DNS answer cache and the mode-first dispatch decision
//! for `ExternalName` backends.
//!
//! The gateway can resolve an `ExternalName` service itself (self-resolution)
//! rather than relying on the controller to publish an EndpointSlice. Which path
//! a request takes is chosen by the `edgion.io/dns-resolution-mode` annotation,
//! decided by the pure [`decide_external_name_path`] (unit-testable, no I/O).
//! Cached self-resolution and AI provider endpoints share
//! [`upstream_dns_cache`], the canonical `edgion_common::dns::DnsAnswerCache`.

use std::sync::LazyLock;
use std::time::Duration;

use crate::cli::config::{get_default_lru_cache_capacity, get_default_lru_cache_shards};
use edgion_common::dns::DnsAnswerCache;

/// Fallback freshness of an upstream answer whose resolver reports no record
/// TTL (EGC `dnsResolver.cacheTtl` default). Kept in sync with the EGC schema
/// default.
pub const DEFAULT_DNS_CACHE_TTL: Duration = Duration::from_secs(5);

/// How the gateway resolves an `ExternalName` backend, from the
/// `edgion.io/dns-resolution-mode` annotation. Defaults to [`Controller`].
///
/// [`Controller`]: DnsResolutionMode::Controller
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DnsResolutionMode {
    /// Re-resolve on every request through the effective resolver; never cache
    /// and never consult a controller-published EndpointSlice.
    PerRequest,
    /// Self-resolve through the effective resolver via the TTL-honoring upstream
    /// answer cache; never consult a controller-published EndpointSlice.
    Gateway,
    /// Prefer a controller-published EndpointSlice; when it has no ready backend,
    /// bridge to self-resolution (cached), so resolution still works before/without
    /// the controller loop.
    #[default]
    Controller,
}

impl DnsResolutionMode {
    /// Parse the annotation value. Unknown / absent values fall back to the
    /// default ([`Controller`]) — an unrecognized mode must not fail the request.
    ///
    /// [`Controller`]: DnsResolutionMode::Controller
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|s| s.trim()) {
            Some("per-request") => DnsResolutionMode::PerRequest,
            Some("gateway") => DnsResolutionMode::Gateway,
            Some("controller") => DnsResolutionMode::Controller,
            _ => DnsResolutionMode::default(),
        }
    }
}

/// The resolution path chosen for a single `ExternalName` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalNamePath {
    /// Route via the controller-published EndpointSlice (`select_service_endpoint`).
    EndpointSlice,
    /// Self-resolve through the effective resolver. `cache = false` for
    /// `per-request` (always re-resolve); `true` for `gateway` and the
    /// `controller`-mode bridge.
    SelfResolve { cache: bool },
}

/// Pure mode-first decision: given the mode and whether the controller slice has
/// at least one ready backend, pick the resolution path.
///
/// - `per-request` → self-resolve, no cache (never consults the slice).
/// - `gateway` → self-resolve, cached (never consults the slice).
/// - `controller` → the slice if it has a ready backend, else bridge to cached
///   self-resolution.
pub fn decide_external_name_path(mode: DnsResolutionMode, slice_has_ready: bool) -> ExternalNamePath {
    match mode {
        DnsResolutionMode::PerRequest => ExternalNamePath::SelfResolve { cache: false },
        DnsResolutionMode::Gateway => ExternalNamePath::SelfResolve { cache: true },
        DnsResolutionMode::Controller => {
            if slice_has_ready {
                ExternalNamePath::EndpointSlice
            } else {
                ExternalNamePath::SelfResolve { cache: true }
            }
        }
    }
}

/// Lower bound on shard count for the upstream answer cache.
///
/// The global `lru_cache` default is `0` (a single shard, one mutex). That is
/// fine for the cold-path caches it was written for, but every AI attempt and
/// every cached `ExternalName` self-resolve request reads this one, and a single
/// mutex would serialize every one of them. This is a floor, not an override: a
/// larger configured shard count wins.
///
/// Be precise about what this buys. An LRU lookup mutates recency, so it takes
/// its shard lock *exclusively* even on a hit — the unbounded concurrent reads
/// of the `RwLock` this replaced are gone, and sharding only spreads contention
/// across *distinct* keys. It does nothing for the shape that matters most: one
/// hot ExternalName Service with one hostname and one resolver is a single key,
/// hashes to a single shard, and now serializes there. That is accepted, not
/// mitigated — the critical section is a hash lookup, an LRU splice and a small
/// `Vec<IpAddr>` copy, and strict bounded replacement is what selects
/// `ShardedLruCache` over `TinyUfo`.
const MIN_DNS_CACHE_SHARDS: usize = 8;

/// Apply the shard floor, clamped to `capacity` (`ShardedLruCache::new` requires
/// `shards <= capacity`).
///
/// At a capacity at or below the floor this yields one entry per shard, so two
/// hash-colliding keys evict each other while other shards sit empty. That is
/// accepted: such a capacity is degenerate for this cache regardless — the
/// default is 1024, and the validated maximum is 65,536.
fn effective_shards(configured: usize, capacity: usize) -> usize {
    configured.max(MIN_DNS_CACHE_SHARDS).min(capacity)
}

/// Process-wide answer cache for data-plane upstream resolution: AI provider
/// endpoints and cached `ExternalName` self-resolution. Sized from the gateway
/// `lru_cache` defaults with the shard floor; kept separate from the egress
/// cache so request-influenced outbound names cannot evict upstream answers.
pub fn upstream_dns_cache() -> &'static DnsAnswerCache {
    static CACHE: LazyLock<DnsAnswerCache> = LazyLock::new(|| {
        let capacity = get_default_lru_cache_capacity();
        DnsAnswerCache::new(capacity, effective_shards(get_default_lru_cache_shards(), capacity))
    });
    &CACHE
}

#[cfg(test)]
#[path = "../../../tests/unit/backends/discovery/dns_cache_test.rs"]
mod tests;
