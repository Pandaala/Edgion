---
id: ARCH-186
status: candidate
kind: architecture-debt
priority: P3
partition: PART-150
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Move the retiring LRU node key instead of cloning it

`edgion-algo/src/cache/lru.rs::LocalLruCacheInner::remove_index` clones the occupied node's `K` solely to remove its corresponding map entry. The function then releases the node, whose reset drops the original key. The removal owns the node's entire retirement under the shard mutex; the original node key is not needed after that operation. The duplicate owned key is discarded immediately rather than retained by another generation or consumer.

This is the canonical release funnel for explicit removal, lazy expiration from get, full-cache expiration pruning and LRU displacement. Those callers all converge on the same private function, so a local ownership correction covers their behavior without changing ShardedLruCache, the public borrowed-key API or any cache policy.

The request-path witness is concrete. Gateway `backends/mod.rs::resolve_external_ips_with` calls the canonical DnsCache get/insert during cached ExternalName self-resolution. DnsCache stores `DnsCacheKey`, whose three fields—service identity, external hostname and resolver fingerprint—are owned Strings. Expired lookup and capacity-pressure insertion reach remove_index and clone that whole key while holding the shard mutex. OIDC request_filter_impl also resolves opaque session references through token_exchange's String-keyed access-token cache; recent-refresh results use another String-keyed canonical cache. These keys are not full bearer tokens. The proposed benefit is removal of one unnecessary key clone per retired entry, including associated String allocations for these actual consumers; no measured latency, RSS, incident or larger hot-path claim is made.

Take the retiring node's existing key into a local owned K before its reset, then use a reference to that key for map removal. Keep the occupied-node guard and missing-key invariant check. Preserve the existing release obligations: remove the exact expiration record before clearing its deadline, detach the same list node, remove the map entry, decrement occupancy once, return the original value and put precisely that slot on the free list. All of these remain in the current mutex critical section. No new cache, closure API, key interning, Arc migration, extra lock or ownership framework is required. Borrowing the key directly for map removal before later node mutations is an alternative, provided Rust's field borrowing and operation order remain clear; moving the already-retired key makes the lifetime explicit.

The key clone during insert has a different ownership requirement: the map and node both retain a key after insertion, so this proposal leaves it alone. Cache hits likewise continue returning the caller-selected cloned V; consumers may choose Arc values as the current contract states. Hashing, shard selection, quotient/remainder capacities, TTL arithmetic, zero-TTL removal, strict replacement, lazy expiration, ordered index, defensive progress guard, approximate stats and cold get_keys sweeps remain unchanged.

The original bounded-expiration-index decision explicitly requires the exact release funnel and its record/list/free-slot invariants, not a key clone. This proposal does not reopen its accepted BTreeSet tradeoff, pruning schedule or counters. The original cache-selection rule continues to require this canonical strict/explicit-invalidation cache; TinyUFO's separately accepted positive-introspection use is unaffected.

All local and sharded LRU test bodies were read, including repeated full-capacity eviction, expired-before-live pruning, rewritten deadlines, explicit removal, clear/free-slot reuse, borrowed String keys and concurrent shard/index bounds. DNS tests additionally cover capacity churn and expired-entry reclamation before live eviction, and OIDC tests cover TTL-backed recent-result freshness and explicit access-token removal. Future verification should retain those behaviors and confirm by source inspection that remove_index retires the original node key without K::clone. No allocator benchmark or production visibility expansion is necessary. Exact source/review searches found no existing finding for this release-key ownership cause. Static read-only review only; no code, tests, builds, network or Git operations were executed or changed.
