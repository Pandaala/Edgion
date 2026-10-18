# ExternalName DNS address-result ownership

Status: deferred optimization for user decision. No runtime change.
Source baseline: d48bb0555.

## Evidence

Native upstream selection reaches resolve_external_ips_with in
edgion-gateway/src/backends/mod.rs. For cached self-resolution, DnsCache::get
clones DnsCacheEntry through the canonical ShardedLruCache/LocalLruCache getter.
The entry owns Vec<IpAddr>, so a nonempty cache hit allocates and copies the
address list while holding the shard mutex. After unlocking, DnsCache checks
its caller-clock deadline. A value that passes the LRU deadline but fails this
second check has already been cloned. On a successful resolver miss, the caller
clones the returned Vec for insertion and keeps its original result.

The caller then scans addresses outside the cache lock using first_safe_inet_addr
in backends/peer_builder.rs. It selects the first safe address in resolver order,
using the captured Service port and the request's allow_loopback_upstream policy.
If all addresses are rejected, it records the last address with the last rejection
status. Empty results return BackendDnsResolutionFailed without a rejected-address
record. IP literals and per-request resolution bypass the cache.

The lookup-key and fixed/prepared-fingerprint allocations have already been
removed. Address cloning is a separate remaining cost; no workload measurements
establish whether it is material or which alternative wins.

## Decision and alternatives

Choose whether to keep independent Vec results or evaluate a shared immutable
result in a new IMPL plan:

- Arc<[IpAddr]> can make cache-hit cloning a reference-count operation and allow
  the miss caller to share the inserted result. Constructing the Arc-backed slice
  from the resolver Vec has conversion/allocation costs. Measure those rather
  than assuming the Vec buffer is reused.
- Arc<Vec<IpAddr>> can move the existing vector into an Arc owner, retaining its
  buffer but adding an outer allocation and indirection. It may retain spare Vec
  capacity that the current insertion clone would not retain. Hit callers share
  atomic reference-count traffic on the same allocation.
- A canonical scoped projection API could select an address while holding the
  cache lock and return a compact outcome. That would move the address scan and
  request-specific safety checks into the critical section. It requires an exact
  cross-crate allow-list and independent review of cache statistics, expiry and
  reentrancy contracts; it is not a DNS-only replacement cache.

An owned-or-shared result could keep literal and cache-disabled results in their
current Vec form. A blanket Arc conversion would also change those paths' costs.
Shared results can remain alive after eviction until callers release them, as
independent copies already can today; assess total memory rather than claiming
cache capacity bounds outstanding results. Arc sharing may reduce aggregate
copies but changes reclamation and contention characteristics. No alternative
has been prototyped or benchmarked in this audit.

## Acceptance for a selected implementation

- Preserve exact address order, first-safe selection, empty-result failure and
  last-rejected-address/status diagnostics. Apply the current request port and
  safety policy each time. Never cache one selected address under the DNS-only
  key: the key intentionally omits those request-specific inputs.
- Preserve complete Service/hostname/resolver identity, pinned configuration
  across await, resolution mode, failure propagation and no negative caching.
  This task does not reopen miss coalescing or add admission/backoff behavior.
- Preserve both deadline checks, insert-time TTL, zero and overflow TTL behavior,
  LRU recency/statistics, shard selection and exact capacity. Keep best-effort
  Service removal semantics; do not assume removal prevents an older in-flight
  lookup from inserting its old answer afterward.
- Keep immutable results alive independently of cache eviction. Never return a
  naked reference after releasing the cache lock, hold a lock across await, or
  introduce unsafe views. If projection is chosen, bound and audit lock-held
  work and prevent callbacks from re-entering the cache.
- Compare allocations, latency, lock hold time, retained capacity and total memory
  for empty/single/multiple-address answers, hits/misses/expiry, repeated hot-key
  access, many keys and concurrent eviction/deletion. Include resolver miss
  insertion and cache-disabled/literal paths, not just isolated Arc::clone.
- Retain existing identity, TTL, capacity, removal, DNS-await and SSRF regressions;
  add ownership-after-eviction and differential outcome tests as appropriate.
  Preserve canonical cache/resolver ownership, access-log diagnostics and redaction.
  No new tracing, metrics, schema knobs or TLS admission changes are implied.

## Original knowledge and verification

The identity/retention contract remains
skills/04-review/backends/regression-guards/externalname-dns-cache-identity-and-lifecycle.md.
The accepted miss behavior remains
skills/04-review/backends/intentional-design/externalname-dns-miss-coalescing-not-implemented.md.
Both were checked against the current implementation; neither proves a new result
representation correct.

Fresh verification of unchanged code passed 47 canonical-cache, 20 DNS-cache,
four first-safe-address and two native DNS await/cache-boundary tests. Formatting,
workspace check/Clippy and agent-document/diff validation passed. No full backend
suite, live transport, result-ownership prototype or performance benchmark ran.
The separately recorded TLS admission fixture failure remains outside this audit.
