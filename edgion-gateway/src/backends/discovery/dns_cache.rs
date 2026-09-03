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
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cache::ShardedLruCache;
use crate::cli::config::{get_default_lru_cache_capacity, get_default_lru_cache_shards};

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
    /// Route via the controller-published EndpointSlice (`select_from_endpoint_slice`).
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
///   fresh from the `ServiceStore` on every request, so a Service whose
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
pub fn get_global_dns_cache() -> Arc<DnsCache> {
    static STORE: std::sync::LazyLock<Arc<DnsCache>> = std::sync::LazyLock::new(|| Arc::new(DnsCache::new()));
    STORE.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TTL long enough that no test hits it accidentally.
    const SEC5: Duration = Duration::from_secs(5);
    /// For entries that must stay live regardless of how long the surrounding
    /// test run stalls: the entry-level check uses the caller's fake `now`, but
    /// the LRU stamps its own deadline from the real clock.
    const MIN1: Duration = Duration::from_secs(60);

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    // ── mode parsing ──────────────────────────────────────────────────────────

    #[test]
    fn mode_parse_defaults_to_controller() {
        assert_eq!(DnsResolutionMode::parse(None), DnsResolutionMode::Controller);
        assert_eq!(DnsResolutionMode::parse(Some("bogus")), DnsResolutionMode::Controller);
        assert_eq!(DnsResolutionMode::parse(Some("  gateway ")), DnsResolutionMode::Gateway);
        assert_eq!(
            DnsResolutionMode::parse(Some("per-request")),
            DnsResolutionMode::PerRequest
        );
    }

    // ── decision function ─────────────────────────────────────────────────────

    #[test]
    fn per_request_never_consults_slice() {
        // Even with a ready slice, per-request self-resolves without caching.
        assert_eq!(
            decide_external_name_path(DnsResolutionMode::PerRequest, true),
            ExternalNamePath::SelfResolve { cache: false }
        );
        assert_eq!(
            decide_external_name_path(DnsResolutionMode::PerRequest, false),
            ExternalNamePath::SelfResolve { cache: false }
        );
    }

    #[test]
    fn gateway_never_consults_slice_and_caches() {
        assert_eq!(
            decide_external_name_path(DnsResolutionMode::Gateway, true),
            ExternalNamePath::SelfResolve { cache: true }
        );
        assert_eq!(
            decide_external_name_path(DnsResolutionMode::Gateway, false),
            ExternalNamePath::SelfResolve { cache: true }
        );
    }

    #[test]
    fn controller_uses_slice_when_ready_else_bridges() {
        assert_eq!(
            decide_external_name_path(DnsResolutionMode::Controller, true),
            ExternalNamePath::EndpointSlice
        );
        assert_eq!(
            decide_external_name_path(DnsResolutionMode::Controller, false),
            ExternalNamePath::SelfResolve { cache: true }
        );
    }

    // ── cache ───────────────────────────────────────────────────────────

    /// Cache with a generous bound, so a test exercises identity and TTL rather
    /// than capacity eviction.
    fn cache() -> DnsCache {
        DnsCache::with_capacity(64, 4)
    }

    #[test]
    fn effective_shards_applies_the_floor_without_exceeding_capacity() {
        // `DnsCache::new()` is only reachable through the global LazyLock, so no
        // other test executes this arithmetic — and it is exactly the expression
        // whose failure mode is a `ShardedLruCache::new` panic at startup.
        for (configured, capacity, expected) in [
            // At or below the floor (including the production default of 0).
            (0, 1024, MIN_DNS_CACHE_SHARDS),
            (1, 1024, MIN_DNS_CACHE_SHARDS),
            (4, 1024, MIN_DNS_CACHE_SHARDS),
            // A larger configured count wins.
            (32, 1024, 32),
            (64, 65_536, 64),
            // A capacity at or below the floor clamps to the capacity.
            (0, 1, 1),
            (0, 2, 2),
            (0, 7, 7),
            (0, 8, MIN_DNS_CACHE_SHARDS),
        ] {
            let shards = effective_shards(configured, capacity);
            assert_eq!(
                shards, expected,
                "effective_shards({configured}, {capacity}) should be {expected}"
            );
            // The invariant ShardedLruCache::new asserts on.
            assert!(shards >= 1 && shards <= capacity, "shards must be in 1..=capacity");
            // Construct a cache at a capped capacity (so the test does not
            // allocate a 65k-node pool) with the clamp recomputed for that
            // capacity — a smoke check that the clamp yields a constructible
            // pair. The `shards <= capacity` assert above is what actually
            // guards the row under test.
            let capped = capacity.min(64);
            let _ = DnsCache::with_capacity(capped, effective_shards(configured, capped));
        }
    }

    #[test]
    fn cache_hit_within_ttl() {
        let cache = cache();
        let now = Instant::now();
        cache.insert("ns/svc", "a.example.com", "fp1", vec![ip("1.2.3.4")], SEC5, now);
        let got = cache.get("ns/svc", "a.example.com", "fp1", now + Duration::from_secs(1));
        assert_eq!(got, Some(vec![ip("1.2.3.4")]));
    }

    #[test]
    fn cache_miss_after_expiry() {
        let cache = cache();
        let now = Instant::now();
        cache.insert("ns/svc", "a.example.com", "fp1", vec![ip("1.2.3.4")], SEC5, now);
        assert_eq!(
            cache.get("ns/svc", "a.example.com", "fp1", now + Duration::from_secs(6)),
            None
        );
    }

    #[test]
    fn cache_miss_exactly_at_the_deadline() {
        // The entry check is `expires_at > now`, so the deadline instant itself
        // is already a miss. Pins the comparison against a `>=` slip.
        let cache = cache();
        let now = Instant::now();
        cache.insert("ns/svc", "a.example.com", "fp1", vec![ip("1.2.3.4")], SEC5, now);
        assert_eq!(
            cache.get("ns/svc", "a.example.com", "fp1", now + SEC5 - Duration::from_nanos(1)),
            Some(vec![ip("1.2.3.4")])
        );
        assert_eq!(cache.get("ns/svc", "a.example.com", "fp1", now + SEC5), None);
    }

    #[test]
    fn cache_miss_on_fingerprint_change() {
        let cache = cache();
        let now = Instant::now();
        cache.insert("ns/svc", "a.example.com", "fp1", vec![ip("1.2.3.4")], SEC5, now);
        // A different resolver fingerprint must not hit the old entry.
        assert_eq!(cache.get("ns/svc", "a.example.com", "fp2", now), None);
    }

    #[test]
    fn cache_miss_on_external_name_change() {
        // The core identity regression: one Service repointed from hostname A to
        // hostname B must never be served A's addresses, TTL notwithstanding.
        let cache = cache();
        let now = Instant::now();
        cache.insert("ns/svc", "a.example.com", "fp1", vec![ip("1.2.3.4")], SEC5, now);
        assert_eq!(cache.get("ns/svc", "b.example.com", "fp1", now), None);
        // The new hostname resolves and caches independently of the old one.
        cache.insert("ns/svc", "b.example.com", "fp1", vec![ip("5.6.7.8")], SEC5, now);
        assert_eq!(
            cache.get("ns/svc", "b.example.com", "fp1", now),
            Some(vec![ip("5.6.7.8")])
        );
    }

    #[test]
    fn distinct_fingerprints_for_same_service_coexist() {
        // Two effective resolvers for one service (e.g. two Gateways) must NOT
        // evict each other — both entries live until their own TTL.
        let cache = cache();
        let now = Instant::now();
        cache.insert("ns/svc", "a.example.com", "fp1", vec![ip("1.2.3.4")], SEC5, now);
        cache.insert("ns/svc", "a.example.com", "fp2", vec![ip("5.6.7.8")], SEC5, now);
        assert_eq!(
            cache.get("ns/svc", "a.example.com", "fp1", now),
            Some(vec![ip("1.2.3.4")])
        );
        assert_eq!(
            cache.get("ns/svc", "a.example.com", "fp2", now),
            Some(vec![ip("5.6.7.8")])
        );
    }

    #[test]
    fn reinsert_same_key_refreshes_entry() {
        let cache = cache();
        let now = Instant::now();
        cache.insert("ns/svc", "a.example.com", "fp1", vec![ip("1.2.3.4")], SEC5, now);
        // Re-resolve under the same identity overwrites with the new address set.
        cache.insert("ns/svc", "a.example.com", "fp1", vec![ip("9.9.9.9")], SEC5, now);
        assert_eq!(
            cache.get("ns/svc", "a.example.com", "fp1", now),
            Some(vec![ip("9.9.9.9")])
        );
        assert_eq!(cache.len(), 1);
    }

    // ── lifecycle ────────────────────────────────────────────────────

    #[test]
    fn invalidate_services_drops_only_the_named_services() {
        let cache = cache();
        let now = Instant::now();
        // Two hostnames and two resolvers under the doomed Service, plus a
        // bystander Service that must survive.
        cache.insert("ns/gone", "a.example.com", "fp1", vec![ip("1.1.1.1")], SEC5, now);
        cache.insert("ns/gone", "b.example.com", "fp1", vec![ip("2.2.2.2")], SEC5, now);
        cache.insert("ns/gone", "a.example.com", "fp2", vec![ip("3.3.3.3")], SEC5, now);
        cache.insert("ns/kept", "c.example.com", "fp1", vec![ip("4.4.4.4")], SEC5, now);
        assert_eq!(cache.len(), 4);

        cache.invalidate_services(&HashSet::from(["ns/gone".to_string()]));

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get("ns/gone", "a.example.com", "fp1", now), None);
        assert_eq!(cache.get("ns/gone", "b.example.com", "fp1", now), None);
        assert_eq!(cache.get("ns/gone", "a.example.com", "fp2", now), None);
        assert_eq!(
            cache.get("ns/kept", "c.example.com", "fp1", now),
            Some(vec![ip("4.4.4.4")])
        );
    }

    #[test]
    fn invalidate_services_is_a_noop_for_an_empty_set() {
        let cache = cache();
        let now = Instant::now();
        cache.insert("ns/svc", "a.example.com", "fp1", vec![ip("1.2.3.4")], SEC5, now);
        cache.invalidate_services(&HashSet::new());
        assert_eq!(
            cache.get("ns/svc", "a.example.com", "fp1", now),
            Some(vec![ip("1.2.3.4")])
        );
    }

    #[test]
    fn capacity_bounds_growth_across_unique_services() {
        // Single shard so capacity is exact rather than per-shard.
        let cache = DnsCache::with_capacity(4, 1);
        let now = Instant::now();
        for index in 0..50 {
            let service_key = format!("ns/svc-{index}");
            cache.insert(&service_key, "a.example.com", "fp1", vec![ip("1.2.3.4")], SEC5, now);
        }
        assert_eq!(cache.len(), 4);
        // The oldest inserts were evicted; the most recent survive.
        assert_eq!(cache.get("ns/svc-0", "a.example.com", "fp1", now), None);
        assert_eq!(
            cache.get("ns/svc-49", "a.example.com", "fp1", now),
            Some(vec![ip("1.2.3.4")])
        );
    }

    #[test]
    fn capacity_bounds_growth_across_resolver_and_hostname_churn() {
        let cache = DnsCache::with_capacity(4, 1);
        let now = Instant::now();
        for index in 0..50 {
            // One Service whose hostname and resolver both keep changing.
            let external_name = format!("h{index}.example.com");
            let fingerprint = format!("fp{index}");
            cache.insert("ns/svc", &external_name, &fingerprint, vec![ip("1.2.3.4")], SEC5, now);
        }
        assert_eq!(cache.len(), 4);
    }

    #[test]
    fn expired_entry_is_reclaimed_in_preference_to_evicting_a_live_one() {
        // The LRU receives the same TTL, so when the cache is full it prunes the
        // expired entry rather than displacing a live one. Reclamation is lazy:
        // this asserts the behavior under capacity pressure, which is where the
        // retention bound is actually enforced.
        let cache = DnsCache::with_capacity(2, 1);
        let now = Instant::now();
        // Order matters: `ns/live` goes in FIRST so it is the LRU tail. If the
        // TTL were not handed to the LRU, `prune_expired` would find nothing and
        // the third insert would displace the tail — evicting the live entry and
        // failing this test. Inserting the stale entry first would make it both
        // the expired entry and the eviction victim, which plain LRU removes
        // either way, and the test would prove nothing.
        // A 60s TTL (not SEC5) keeps the live entry clear of the LRU's real-clock
        // deadline no matter how long the surrounding test run stalls.
        cache.insert("ns/live", "b.example.com", "fp1", vec![ip("2.2.2.2")], MIN1, now);
        cache.insert(
            "ns/stale",
            "a.example.com",
            "fp1",
            vec![ip("1.1.1.1")],
            Duration::from_millis(20),
            now,
        );
        assert_eq!(cache.len(), 2);

        std::thread::sleep(Duration::from_millis(40));
        let later = Instant::now();
        cache.insert("ns/new", "c.example.com", "fp1", vec![ip("3.3.3.3")], MIN1, later);

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("ns/stale", "a.example.com", "fp1", later), None);
        // The live entry was not displaced to make room for the new one.
        assert_eq!(
            cache.get("ns/live", "b.example.com", "fp1", later),
            Some(vec![ip("2.2.2.2")])
        );
        assert_eq!(
            cache.get("ns/new", "c.example.com", "fp1", later),
            Some(vec![ip("3.3.3.3")])
        );
    }

    #[test]
    fn concurrent_readers_and_invalidation_stay_consistent() {
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(DnsCache::with_capacity(256, 8));
        let now = Instant::now();
        // Each key gets its OWN address so a reader can assert it got back the
        // value belonging to the key it asked for. A single shared value would
        // make the assertion unfalsifiable and would not catch a shard-index or
        // key-hashing mistake.
        for index in 0..32 {
            cache.insert(
                &format!("ns/svc-{index}"),
                "a.example.com",
                "fp1",
                vec![ip(&format!("10.0.0.{index}"))],
                MIN1,
                now,
            );
        }

        let mut handles = Vec::new();
        for _ in 0..4 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for _ in 0..200 {
                    for index in 0..32 {
                        // Either this key's own address set or nothing — never
                        // another key's value.
                        let got = cache.get(&format!("ns/svc-{index}"), "a.example.com", "fp1", Instant::now());
                        if let Some(addrs) = got {
                            assert_eq!(addrs, vec![ip(&format!("10.0.0.{index}"))]);
                        }
                    }
                }
            }));
        }
        {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for index in 0..32 {
                    cache.invalidate_services(&HashSet::from([format!("ns/svc-{index}")]));
                }
            }));
        }
        for handle in handles {
            handle.join().expect("worker thread panicked");
        }
        assert_eq!(cache.len(), 0);
    }
}
