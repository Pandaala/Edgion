//! Request-driven upstream DNS answer cache for AI provider endpoints and
//! external jumps, whose hostnames are chosen per request (the Envoy
//! dynamic-forward-proxy shape). `ExternalName` Services do not use it: they
//! read the background-refreshed endpoint table (`dns_table`) or, in `system`
//! mode, ask the OS resolver directly.

use std::sync::LazyLock;
use std::time::Duration;

use crate::cli::config::{get_default_lru_cache_capacity, get_default_lru_cache_shards};
use edgion_common::dns::DnsAnswerCache;

/// Fallback freshness of an upstream answer whose resolver reports no record
/// TTL (EGC `dnsResolver.fallbackRefreshSeconds` default), used when no EGC
/// defines `dnsResolver`.
pub const DEFAULT_DNS_CACHE_TTL: Duration = Duration::from_secs(
    edgion_resources::resources::edgion_gateway_config::DNS_DEFAULT_FALLBACK_REFRESH_SECONDS as u64,
);

/// Lower bound on shard count for the upstream answer cache.
///
/// The global `lru_cache` default is `0` (a single shard, one mutex). That is
/// fine for the cold-path caches it was written for, but every AI attempt and
/// every external-jump request reads this one, and a single
/// mutex would serialize every one of them. This is a floor, not an override: a
/// larger configured shard count wins.
///
/// Be precise about what this buys. An LRU lookup mutates recency, so it takes
/// its shard lock *exclusively* even on a hit — the unbounded concurrent reads
/// of the `RwLock` this replaced are gone, and sharding only spreads contention
/// across *distinct* keys. It does nothing for the shape that matters most: one
/// hot upstream hostname with one resolver is a single key,
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
/// endpoints and external jumps. Sized from the gateway
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
