//! Data-plane DNS resolution cache and the mode-first dispatch decision for
//! `ExternalName` backends.
//!
//! The gateway can resolve an `ExternalName` service itself (self-resolution)
//! rather than relying on the controller to publish an EndpointSlice. Which path
//! a request takes is chosen by the `edgion.io/dns-resolution-mode` annotation,
//! decided by the pure [`decide_external_name_path`] (unit-testable, no I/O), and
//! self-resolved addresses are memoized for a short TTL by [`DnsCache`] so the
//! hot path does not re-resolve on every request.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

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

/// Cache key: a resolved address set is valid only for a given service *and* a
/// given resolver identity (see [`edgion_common::dns::ResolverSpec::fingerprint`]),
/// so a resolver change invalidates rather than serving stale addresses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DnsCacheKey {
    service_key: String,
    fingerprint: String,
}

#[derive(Debug, Clone)]
struct DnsCacheEntry {
    addrs: Vec<IpAddr>,
    expires_at: Instant,
}

/// Short-TTL cache of self-resolved `ExternalName` addresses.
#[derive(Default)]
pub struct DnsCache {
    entries: RwLock<HashMap<DnsCacheKey, DnsCacheEntry>>,
}

impl DnsCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the cached addresses for `(service_key, fingerprint)` if the entry
    /// is present and not expired as of `now`. An expired entry is treated as a
    /// miss (lazily overwritten on the next [`insert`](Self::insert)).
    pub fn get(&self, service_key: &str, fingerprint: &str, now: Instant) -> Option<Vec<IpAddr>> {
        let key = DnsCacheKey {
            service_key: service_key.to_string(),
            fingerprint: fingerprint.to_string(),
        };
        let guard = self.entries.read();
        let entry = guard.get(&key)?;
        if entry.expires_at > now {
            Some(entry.addrs.clone())
        } else {
            None
        }
    }

    /// Store `addrs` for `(service_key, fingerprint)`, expiring at `now + ttl`.
    ///
    /// A *live* entry under a different fingerprint is kept: the same `service_key`
    /// can legitimately carry distinct effective resolvers at once (e.g. fronted by
    /// two Gateways with different `edgion.io/dns-resolver` annotations), and
    /// evicting those would make them thrash each other's cache every request.
    /// Only *expired* entries for this same `service_key` are opportunistically
    /// dropped here — that bounds accumulation across genuine resolver changes
    /// without a background sweep (there is otherwise no GC; `get` treats expired
    /// as a miss but never removes).
    pub fn insert(&self, service_key: &str, fingerprint: &str, addrs: Vec<IpAddr>, ttl: Duration, now: Instant) {
        let key = DnsCacheKey {
            service_key: service_key.to_string(),
            fingerprint: fingerprint.to_string(),
        };
        let mut guard = self.entries.write();
        guard.retain(|k, v| k.service_key != service_key || v.expires_at > now);
        guard.insert(
            key,
            DnsCacheEntry {
                addrs,
                expires_at: now + ttl,
            },
        );
    }

    /// Number of live-or-stale entries currently held (test-only; used to assert
    /// the opportunistic GC in [`insert`](Self::insert)).
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.read().len()
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

    // ── cache ─────────────────────────────────────────────────────────────────

    #[test]
    fn cache_hit_within_ttl() {
        let cache = DnsCache::new();
        let now = Instant::now();
        cache.insert("ns/svc", "fp1", vec![ip("1.2.3.4")], Duration::from_secs(5), now);
        let got = cache.get("ns/svc", "fp1", now + Duration::from_secs(1));
        assert_eq!(got, Some(vec![ip("1.2.3.4")]));
    }

    #[test]
    fn cache_miss_after_expiry() {
        let cache = DnsCache::new();
        let now = Instant::now();
        cache.insert("ns/svc", "fp1", vec![ip("1.2.3.4")], Duration::from_secs(5), now);
        assert_eq!(cache.get("ns/svc", "fp1", now + Duration::from_secs(6)), None);
    }

    #[test]
    fn cache_miss_on_fingerprint_change() {
        let cache = DnsCache::new();
        let now = Instant::now();
        cache.insert("ns/svc", "fp1", vec![ip("1.2.3.4")], Duration::from_secs(5), now);
        // A different resolver fingerprint must not hit the old entry.
        assert_eq!(cache.get("ns/svc", "fp2", now), None);
    }

    #[test]
    fn distinct_fingerprints_for_same_service_coexist() {
        // Two effective resolvers for one service (e.g. two Gateways) must NOT
        // evict each other — both entries live until their own TTL.
        let cache = DnsCache::new();
        let now = Instant::now();
        cache.insert("ns/svc", "fp1", vec![ip("1.2.3.4")], Duration::from_secs(5), now);
        cache.insert("ns/svc", "fp2", vec![ip("5.6.7.8")], Duration::from_secs(5), now);
        assert_eq!(cache.get("ns/svc", "fp1", now), Some(vec![ip("1.2.3.4")]));
        assert_eq!(cache.get("ns/svc", "fp2", now), Some(vec![ip("5.6.7.8")]));
    }

    #[test]
    fn insert_gcs_expired_same_service_entries_but_keeps_live_ones() {
        let cache = DnsCache::new();
        let now = Instant::now();
        // fp1: short TTL, will be expired by the third insert.
        cache.insert("ns/svc", "fp1", vec![ip("1.1.1.1")], Duration::from_secs(1), now);
        // fp2: long TTL, still live at the third insert (a different Gateway's resolver).
        cache.insert("ns/svc", "fp2", vec![ip("2.2.2.2")], Duration::from_secs(30), now);
        assert_eq!(cache.len(), 2);

        let later = now + Duration::from_secs(2);
        cache.insert("ns/svc", "fp3", vec![ip("3.3.3.3")], Duration::from_secs(30), later);
        // fp1 (expired) was GC'd on the fp3 insert; fp2 (live) + fp3 remain.
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("ns/svc", "fp2", later), Some(vec![ip("2.2.2.2")]));
        assert_eq!(cache.get("ns/svc", "fp3", later), Some(vec![ip("3.3.3.3")]));
        assert_eq!(cache.get("ns/svc", "fp1", later), None);
    }

    #[test]
    fn reinsert_same_fingerprint_refreshes_entry() {
        let cache = DnsCache::new();
        let now = Instant::now();
        cache.insert("ns/svc", "fp1", vec![ip("1.2.3.4")], Duration::from_secs(5), now);
        // Re-resolve under the same fingerprint overwrites with the new address set.
        cache.insert("ns/svc", "fp1", vec![ip("9.9.9.9")], Duration::from_secs(5), now);
        assert_eq!(cache.get("ns/svc", "fp1", now), Some(vec![ip("9.9.9.9")]));
    }
}
