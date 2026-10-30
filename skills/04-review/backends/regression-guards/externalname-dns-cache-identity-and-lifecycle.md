---
name: externalname-dns-cache-identity-and-lifecycle
description: Use when reviewing the Gateway ExternalName DNS cache; covers what belongs in its key, why a Service update needs no explicit invalidation, and how its retention is bounded.
status: fixed
finding_id: task-externalname-dns-cache-identity-and-lifecycle
closed: 2026-09-03
knowledge_type: regression-guards
---

# ExternalName DNS Cache Identity and Lifecycle (task-externalname-dns-cache-identity-and-lifecycle, origin D0001) — FIXED

## Conclusion

`DnsCache` keyed self-resolved `ExternalName` addresses by `(service_key, fingerprint)` only.
Historically, the queried hostname was read from `ServiceStore` but was **not** part
of the key, so repointing a Service from hostname A to hostname B served A's addresses until TTL
expiry — a window whose length is operator-controlled (`EdgionGatewayConfig.spec.dnsResolver.cacheTtl`
is a free-form duration, not capped at the 5s default). The backing store was also an unbounded
`HashMap` with no removal path for deleted Services.

The key is now the complete identity `(service_key, external_name, fingerprint, family)`, the store is a
capacity-bounded `ShardedLruCache`, and the Service conf-handler invalidates on removal.

Native HTTP/gRPC now capture the actual Service source, checked section and
membership-matched runtime once per attempt. The hostname and resolver inputs
remain borrowed from that capture through DNS completion. Current cache identity
does not require a resourceVersion component because hostname, resolver and family
identity describe the address answer; new evidence can change that conclusion.

## Core Rationale

**1. The key must name every input that can change the resolved address set**

Four inputs, all now in `DnsCacheKey`:

- `service_key` — ownership, and the unit of invalidation on Service removal.
- `external_name` — the hostname actually handed to the resolver. This was the defect: it varied
  per Service generation while the key did not.
- `family` — the captured `DnsLookupFamily`; a live family change cannot reuse the old policy answer, even with a long TTL.
- `fingerprint` — `ResolverSpec::fingerprint()`, so a resolver change cannot serve addresses
  resolved by a different resolver.

The key is built from the captured hostname and resolver used by the actual query,
so Service replacement cannot mix old geometry with a new DNS query. `DnsCache::get`/`insert` take
`external_name` as a required parameter, so the wiring cannot silently regress — a future refactor
would have to actively pass the wrong value.

**2. A Service *update* needs no explicit invalidation; only *deletion* does**

When hostname, resolver or family changes, a new attempt uses a different key. An in-flight
old attempt may still complete and populate its old key, including after deletion;
that bounded entry cannot answer the new hostname/resolver. Unchanged content may
reuse the cache across Service generations. Old entries expire or leave by capacity. Adding
update-time invalidation would buy nothing for correctness while forcing a DNS re-resolution after
any unrelated Service spec change (a port edit, a label edit) that happens to flow through
conf_sync.

Deletion is different: the entry is likewise unreachable, but nothing bounds *when* it is reclaimed
— it can remain until lazy expiry or unrelated eviction. The Service conf-handler
calls `DnsCache::invalidate_services` for removed keys after discovery publication.
This is prompt best-effort reclamation, not an atomic barrier against an in-flight
old attempt inserting behind removal.

The sweep walks the live key set, so an entry that had already expired at deletion time keeps its
slot until capacity pressure prunes it. Prompt reclamation is therefore a claim about live entries;
expired ones are unreachable regardless, and only the moment of reclamation differs.

**3. Bounding is the cache's job, not a per-insert sweep's**

The old `insert` ran `retain(|k, v| k.service_key != service_key || v.expires_at > now)` — an
opportunistic GC that only ever touched the *same* `service_key`. Entries for deleted Services had
no reclamation path at all, so churn across unique Service keys grew the map for the process
lifetime. `ShardedLruCache` supplies the documented bound (from the gateway's
`lru_cache` defaults, the same source the OIDC access-token cache uses). Its explicit `remove` is
what makes deletion-invalidation possible; per the in-process cache selection rule this is exactly
the case `ShardedLruCache` exists for and `TinyUfo` does not fit.

Expiry stays lazy: the TTL is handed to the LRU as well as recorded on the entry, so under capacity
pressure the expiration index drops stale entries in preference to evicting live ones. A stale entry
may hold its slot until the cache needs it. That costs bounded memory and never a stale hit.

**4. The cache is sharded even when the global default is not**

`lru_cache.default_shards` is `0` (one shard, one mutex), which suits the cold-path caches it was
written for. This cache is read by every `ExternalName` self-resolve request, and the store it
replaced was an `RwLock` whose reads ran concurrently. `DnsCache::new` therefore applies a shard
floor (`MIN_DNS_CACHE_SHARDS`, clamped to the capacity) so the move to an LRU does not turn a
shared-read hot path into a single serialized mutex. A larger configured shard count still wins.
Be precise about what the floor buys: an LRU lookup mutates recency, so it takes its shard lock
exclusively even on a hit, and sharding only spreads contention across *distinct* keys. The shape
that matters most — one hot ExternalName Service, one hostname, one resolver — is a single key on a
single shard and now serializes there. That is accepted rather than mitigated: the critical section
is a hash lookup, an LRU splice and a small `Vec<IpAddr>` clone, and per-key removal is a hard
requirement that rules out an admission-based cache. The clamp is covered by
`effective_shards_applies_the_floor_without_exceeding_capacity`, because `DnsCache::new` is reachable
only through the global `LazyLock` and no other test executes that arithmetic.

**5. Distinct live resolvers for one Service still coexist**

One `service_key` can legitimately carry different effective resolvers at once (two Gateways with
different `edgion.io/dns-resolver` annotations). Those are separate keys and neither evicts the
other; only the shared capacity bound applies.

## Previously considered alternatives

- **Add a Service generation / `resourceVersion` to the key** — redundant. The generation is only
  observable through the fields that actually affect resolution, and all are already in the key;
  adding it would additionally invalidate on spec changes that cannot change the answer.
- **Invalidate on Service update as well as delete** — no correctness benefit (the old entry is
  already unreachable), and it converts any unrelated Service spec change into a forced DNS
  re-resolution on the request path.
- **Drop the shard floor and take `lru_cache.default_shards` verbatim** — the default is `0`, which
  would serialize every ExternalName self-resolve request on one mutex; the store this replaced
  allowed concurrent reads.
- **Add a background sweep / timer to evict expired entries** — the retention bound is the LRU
  capacity, which holds without one. A timer would add a task and a wakeup for memory that is
  already bounded.
- **Keep the `HashMap` and extend the opportunistic `retain` sweep** — it cannot reach entries for
  deleted Services, which is the unbounded case, and it is O(n) under the write lock.
- **Use `TinyUfo`/`TinyUfoTtlCache` for the bound** — deletion-invalidation needs explicit per-key
  removal, which TinyUFO does not provide.

## Re-evaluation Triggers

Re-evaluate against current source and new evidence, including:

- ExternalName resolution gains an input that is neither the Service key, the hostname, nor the
  resolver identity (it would have to enter the key).
- Miss coalescing is reopened (declined 2026-09-03, see
  [externalname-dns-miss-coalescing-not-implemented.md](../intentional-design/externalname-dns-miss-coalescing-not-implemented.md))
  and its in-flight state needs an ownership rule that interacts with this key.
- Measured DNS load shows LRU capacity eviction, not TTL, is driving re-resolution — the capacity
  would then need its own knob rather than the shared `lru_cache` default.
- Lock contention on a single hot ExternalName key shows up in a profile. Sharding cannot help that
  shape; the answer would be a read-mostly structure that does not mutate on lookup (which then has
  to solve per-key invalidation some other way).
- A deployment sets `lru_cache.default_capacity` at or below `MIN_DNS_CACHE_SHARDS`. The clamp then
  gives one entry per shard, so two hash-colliding keys evict each other while other shards sit
  empty. Accepted today because such a capacity is degenerate for this cache regardless (the default
  is 1024); a real deployment doing it would justify a per-shard minimum-occupancy rule instead.

## Reference Cases

  promoted to a `tasks/todo` entry on 2026-09-01 and removed on closure; fixed 2026-09-03.
- Source: `edgion-gateway/src/backends/discovery/dns_cache.rs`,
  `edgion-gateway/src/backends/discovery/services/conf_handler_impl.rs`,
  `edgion-gateway/src/backends/mod.rs` (`resolve_external_ips_with`).
- Cache selection rule: `skills/01-architecture/00-common/05-unification-points.md`.
- Related but separate: miss coalescing / failure backoff, declined 2026-09-03 —
  [externalname-dns-miss-coalescing-not-implemented.md](../intentional-design/externalname-dns-miss-coalescing-not-implemented.md).

## Resolution references

- `task-externalname-dns-cache-identity-and-lifecycle` — 2026-09-03.

## Address-family policy extension

ISSUE-008 introduced the shared family policy while fixing Controller owned-slice
convergence. Resolver choice and family are captured together before a DNS await.
The cache key uses that captured family for both lookup and insertion; an old
in-flight completion populates only its old bounded key. Deletion still invalidates
all Service keys. Family changes do not alter prepared HTTPDNS TLS/client identity.
Service explicit family gates only ExternalName canonical membership; ordinary
Service slices remain unfiltered. Gateway/EGC defaults belong to self-resolution
and AI, not shared Controller-produced slices. Historical verification above is
not fresh evidence for this extension; current targeted family tests and final
verification are recorded in the active implementation task.
