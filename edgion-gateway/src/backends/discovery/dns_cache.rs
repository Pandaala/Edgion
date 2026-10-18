//! Data-plane DNS resolution cache and the mode-first dispatch decision for
//! `ExternalName` backends.
//!
//! The gateway can resolve an `ExternalName` service itself (self-resolution)
//! rather than relying on the controller to publish an EndpointSlice. Which path
//! a request takes is chosen by the `edgion.io/dns-resolution-mode` annotation,
//! decided by the pure [`decide_external_name_path`] (unit-testable, no I/O), and
//! self-resolved addresses are memoized for a short TTL by [`DnsCache`] so the
//! hot path does not re-resolve on every request.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use crate::cli::config::{get_default_lru_cache_capacity, get_default_lru_cache_shards};
use edgion_algo::cache::ShardedLruCache;

/// Default TTL for a self-resolved address set (EGC `dnsResolver.cacheTtl`
/// default). Kept in sync with the EGC schema default.
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
    /// Self-resolve through the effective resolver with a short-TTL cache; never
    /// consult a controller-published EndpointSlice.
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

/// Cache key: the complete identity of a self-resolved address set.
///
/// Every input that can change the resolved addresses is part of the key:
///
/// - `service_key` — which Service the entry belongs to, and the unit of
///   invalidation when that Service is removed.
/// - `external_name` — the hostname actually handed to the resolver. It is read
///   from the pinned Service discovery projection for each attempt, so a Service whose
///   `spec.externalName` changes produces a *different* key and can never be
///   served the previous hostname's addresses.
/// - `fingerprint` — the effective resolver identity (see
///   [`edgion_common::dns::ResolverSpec::fingerprint`]), so a resolver change
///   invalidates rather than serving addresses resolved by another resolver.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DnsCacheKey {
    service_key: String,
    external_name: String,
    fingerprint: String,
}

#[derive(Debug, Clone)]
struct DnsCacheEntry {
    addrs: Vec<IpAddr>,
    expires_at: Instant,
}

/// Short-TTL, capacity-bounded cache of self-resolved `ExternalName` addresses.
///
/// ## Retention contract
///
/// - **Bounded** — entries live in a [`ShardedLruCache`] sized from the gateway's
///   global LRU defaults, so churn across unique Services, hostnames, or resolvers
///   can never grow the cache past that capacity.
/// - **Expired entries are reclaimed, not just skipped** — the TTL is handed to
///   the LRU as well as recorded on the entry, so under capacity pressure its
///   expiration index drops stale entries in preference to evicting live ones.
///   Reclamation is lazy by design (no timer, no background task): a stale entry
///   may hold its slot until the cache actually needs it, which costs bounded
///   memory but never a stale hit.
/// - **Deleted Services are reclaimed promptly** — the Service conf-handler calls
///   [`invalidate_services`](Self::invalidate_services) on removal. That sweep
///   walks the key set the LRU still considers live, so an entry already past
///   the LRU's own deadline at deletion time keeps its slot until capacity
///   pressure prunes it. It is unreachable either way; only the moment of
///   reclamation differs.
/// - **The TTL is stamped at insert, not re-read** — `cacheTtl` is resolved per
///   request but only applies to entries created after the change. Shortening it
///   does not shorten entries already cached; those run out their original
///   deadline. `cacheTtl: "0s"` stores nothing at all (the LRU turns a zero TTL
///   into a removal), which matches the entry check treating `now` as expired.
///   Note the key covers every input that changes the resolved *addresses*; the
///   TTL governs how long an unchanged answer is reused and is deliberately not
///   part of the identity.
/// - **A Service update needs no explicit invalidation** — `external_name` and
///   `fingerprint` are part of the key, so the previous generation's entry is
///   already unreachable and is reclaimed by TTL or capacity.
pub struct DnsCache {
    entries: ShardedLruCache<DnsCacheKey, DnsCacheEntry>,
}

impl Default for DnsCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Lower bound on shard count for this cache.
///
/// The global `lru_cache` default is `0` (a single shard, one mutex). That is
/// fine for the cold-path caches it was written for, but every `ExternalName`
/// self-resolve request reads this one, and a single mutex would serialize every
/// one of them. This is a floor, not an override: a larger configured shard
/// count wins.
///
/// Be precise about what this buys. An LRU lookup mutates recency, so it takes
/// its shard lock *exclusively* even on a hit — the unbounded concurrent reads
/// of the `RwLock` this replaced are gone, and sharding only spreads contention
/// across *distinct* keys. It does nothing for the shape that matters most: one
/// hot ExternalName Service with one hostname and one resolver is a single key,
/// hashes to a single shard, and now serializes there. That is accepted, not
/// mitigated — the critical section is a hash lookup, an LRU splice and a small
/// `Vec<IpAddr>` clone, and per-key removal (which `TinyUfo` cannot do) is
/// required by [`DnsCache::invalidate_services`].
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

impl DnsCache {
    /// Build a cache sized from the gateway's global LRU defaults, with at least
    /// [`MIN_DNS_CACHE_SHARDS`] shards (clamped to the capacity, which
    /// `ShardedLruCache::new` requires).
    pub fn new() -> Self {
        let capacity = get_default_lru_cache_capacity();
        Self::with_capacity(capacity, effective_shards(get_default_lru_cache_shards(), capacity))
    }

    /// Build a cache with an explicit bound. `shards` of `0` or `1` disables sharding.
    pub fn with_capacity(capacity: usize, shards: usize) -> Self {
        Self {
            entries: ShardedLruCache::new(capacity, shards),
        }
    }

    /// Return the cached addresses for this key if the entry is present and not
    /// expired as of `now`.
    ///
    /// The entry's own `expires_at` is the authoritative TTL check; the LRU's TTL
    /// is the reclamation backstop. The two are stamped from different clocks —
    /// `expires_at` from the `now` the caller passed to [`insert`](Self::insert),
    /// the LRU's from its own `Instant::now()` taken slightly later — so they can
    /// disagree by that gap. The disagreement is one-directional and safe: it can
    /// only make a live entry read as a miss, never make an expired one read as a
    /// hit.
    pub fn get(&self, service_key: &str, external_name: &str, fingerprint: &str, now: Instant) -> Option<Vec<IpAddr>> {
        let key = DnsCacheKey {
            service_key: service_key.to_string(),
            external_name: external_name.to_string(),
            fingerprint: fingerprint.to_string(),
        };
        let entry = self.entries.get(&key)?;
        if entry.expires_at > now {
            Some(entry.addrs)
        } else {
            None
        }
    }

    /// Store `addrs` for this key, expiring at `now + ttl`.
    ///
    /// Entries under a different `external_name` or `fingerprint` for the same
    /// `service_key` are left alone: one Service can legitimately carry distinct
    /// effective resolvers at once (e.g. fronted by two Gateways with different
    /// `edgion.io/dns-resolver` annotations), and evicting those would make them
    /// thrash each other's cache every request. Bounding is the LRU's job, not a
    /// per-insert sweep's.
    pub fn insert(
        &self,
        service_key: &str,
        external_name: &str,
        fingerprint: &str,
        addrs: Vec<IpAddr>,
        ttl: Duration,
        now: Instant,
    ) {
        // `checked_add` rather than `+`: this is a public API taking an arbitrary
        // `Duration`, and the layer underneath already guards the same way. A
        // deadline that cannot be represented means the entry can never be
        // served, so skip it instead of panicking on the data plane.
        let Some(expires_at) = now.checked_add(ttl) else {
            return;
        };
        let key = DnsCacheKey {
            service_key: service_key.to_string(),
            external_name: external_name.to_string(),
            fingerprint: fingerprint.to_string(),
        };
        self.entries.insert(key, DnsCacheEntry { addrs, expires_at }, Some(ttl));
    }

    /// Drop every entry belonging to any of `service_keys`.
    ///
    /// Called from the Service conf-handler when Services are removed, so a
    /// deleted Service's addresses are reclaimed immediately instead of waiting
    /// for unrelated traffic to evict them. It runs on the cold conf-sync path
    /// and only for updates that actually remove a Service.
    ///
    /// The scan covers the live key set (bounded by the cache capacity), which
    /// excludes already-expired entries — those are unreachable regardless and
    /// are reclaimed by the LRU's expiration index under capacity pressure.
    pub fn invalidate_services(&self, service_keys: &HashSet<String>) {
        if service_keys.is_empty() {
            return;
        }
        for key in self.entries.get_keys() {
            if service_keys.contains(&key.service_key) {
                self.entries.remove(&key);
            }
        }
    }

    /// Number of occupied entries (test-only). Counts expired-but-not-yet-pruned
    /// entries too — `ShardedLruCache::len` reports slot occupancy, not liveness.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Global data-plane DNS cache (mirrors the other backend stores' global-handle idiom).
pub fn get_global_dns_cache() -> &'static DnsCache {
    static STORE: LazyLock<DnsCache> = LazyLock::new(DnsCache::new);
    &STORE
}

#[cfg(test)]
#[path = "../../../tests/unit/backends/discovery/dns_cache_test.rs"]
mod tests;
