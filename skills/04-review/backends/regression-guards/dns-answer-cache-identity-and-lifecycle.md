---
name: dns-answer-cache-identity-and-lifecycle
description: Use when reviewing DNS answer caching for AI upstreams, ExternalName self-resolution or EgressResolver; covers the shared DnsAnswerCache key, TTL/stale/negative lifecycle, coalescing, bounds and consumer contracts.
status: fixed
finding_id: common-02 (supersedes task-externalname-dns-cache-identity-and-lifecycle, origin D0001)
closed: 2026-11-06
knowledge_type: regression-guards
---

# DNS Answer Cache Identity and Lifecycle (common-02; origin D0001) — FIXED

## Conclusion and scope

All DNS answer caching goes through one canonical mechanism,
`edgion_common::dns::DnsAnswerCache` (`edgion-common/src/dns/cache.rs`). It fronts
`resolve_answer` for two process-wide instances:

| Instance | Consumers | Sizing |
|---|---|---|
| `egress_answer_cache()` (`edgion-common`) | every `EgressResolver` (Gateway plugins, LinkSys, Wasm, GeoIP, Controller OAuth2) | 4096 entries, 16 shards |
| `upstream_dns_cache()` (`edgion-gateway/src/backends/discovery/dns_cache.rs`) | AI provider endpoints, cached ExternalName self-resolution | `lru_cache` defaults with an 8-shard floor |

Callers must not add another DNS cache. The instances are separate so request-influenced egress
names (DSL, redirects) cannot evict upstream answers.

**Key** = `(ResolverSpec::fingerprint(), queried name, DnsQueryPolicy)`. These inputs describe the
answer completely: a changed hostname, resolver or family is a different key, so updates need no
invalidation and deleted Services own nothing (the key carries no Service identity). HTTPDNS
fingerprints include the generation's readiness (`httpdns:ready:` / `httpdns:fallback:`), so a
fallback-only generation never reads a ready generation's answers; secret-only rotation keeps the
fingerprint. StandardDns fingerprints are precomputed in `StandardDnsServers` at configuration
time, so a warm hit allocates nothing for the key.

**Lifecycle** (deadlines stamped at write time from the writer's `DnsCachePolicy`):

- Fresh for the reported record TTL clamped to `[5 s, 300 s]` (floor = Envoy
  `dns_min_refresh_rate`), or for the consumer's fallback TTL (also capped at 300 s) when the
  resolver reports none (System resolver, HTTPDNS without `ttlPath`). Fallback TTL `0` disables
  the cache for that consumer. IP literals and names longer than 254 bytes always bypass it.
- Stale for 30 s afterwards: the first request to claim the entry's `refreshing` flag refreshes
  inline; every other request gets the stale answer without waiting or touching the flight
  registry. A claim on an entry that was replaced is never released, so a reader holding the
  retired entry cannot start a second refresh.
- DNS failure keeps the last good answer (Envoy keeps its last resolution): a transient refresh
  failure keeps serving the stale answer and slides its stale window to `now + 30 s`, so a
  still-requested answer survives an outage of any length while an unused one expires. A refresh
  claimed while the answer was servable keeps it even if the resolver times out after the window
  ended, and answers are retained in the LRU one extra stale window for that reason. Refreshes
  back off 1, 2, 4, 8, then 10 s (cap = Envoy's default 10x base). `NoAddresses` (authoritative
  empty answer, including Standard DNS NXDOMAIN) drops the answer. System-resolver NXDOMAIN, and
  on the `Family` policy (AI, ExternalName) Standard DNS empty answers without SOA evidence,
  surface as `Io`, so for them a removed name keeps being served while it is requested and the
  resolver keeps failing — the accepted cost of Envoy's last-good behavior. The `Native` policy
  (egress, Controller) maps every Standard DNS `NoRecordsFound` to `NoAddresses`.
- Write rule: a failure never overwrites a newer answer. When a lookup fails, it reads the current
  entry; a servable answer written by someone else wins and is served unchanged.
- Cold miss: same-key callers serialize on a per-key flight mutex from the shared
  `edgion_common::singleflight::FlightRegistry` and re-check the cache after acquiring it. A
  cancelled leader releases the mutex and the next waiter takes over. The mutex guards no data
  and only serializes same-key misses — the JWKS-provider shape (exempt from design rule V-1, see
  [data-free single-flight mutex](../../concurrency/intentional-design/single-flight-mutex-across-io.md)).
- A failure with no servable fallback answer is a negative entry for the current back-off
  (1 s, doubling per consecutive failure up to 10 s); the failure count outlives the window so
  the back-off can grow.

**Address selection**: cached answers keep their order for the TTL, so cached ExternalName
lookups (`rotate_addresses`) and AI attempts (`select_ai_addr`) start at the shared process-wide
`next_address_rotation()` offset (`edgion-gateway/src/backends/mod.rs`); AI attempts also prefer
addresses the request has not dialed. `per-request` ExternalName lookups keep the resolver's
order. Exact numeric bounds live in the `edgion-common/src/dns/cache.rs` module docs and the
operator page `docs/en/ops-guide/configuration/edgion-gateway-config.md`.

**Consumer contract**: the cache stores raw answers. Every consumer applies its own egress or
upstream address classification on each use (`filter_resolved_addrs*`), so one answer can serve
consumers with different `blockPrivate` policies and the dialed address is always classified.
`edgion_outbound_egress_blocked_total` therefore counts per use.

## Rationale and evidence

- Finding `common-02` (2026-11-06): resolution was stateless — StandardDns built a hickory resolver
  per call, record TTLs were never read, the AI path resolved on every attempt with no cache, and
  `EgressResolver` kept a 16-entry memo cleared wholesale on overflow. Design:
  `docs/superpowers/specs/2026-11-06-shared-dns-answer-cache-design.md`.
- Historical origin D0001 (2026-09-03): the gateway ExternalName `DnsCache` first lacked the
  hostname in its key and had an unbounded store; the complete-identity and bounded-LRU rules from
  that fix carry over (the per-Service key component and deletion sweep were retired with the
  struct).
- `ShardedLruCache` is selected for strict bounded replacement; `TinyUfo` stays reserved for its
  approved use. An LRU lookup mutates recency, so a single hot key serializes on one shard
  (critical section: hash lookup, LRU splice, small `Vec` copy) — accepted.

## Alternatives and limits

- Long-lived hickory resolvers were rejected: a resolver is built once per miss per key; a
  long-lived one adds a second TTL cache inside hickory and binds its background tasks to the
  first Pingora runtime that used it.
- A spawned leader with a watch-channel registry was rejected for the knowledge-page shape
  (mutex + re-check): no detached tasks, no drop-guard protocol, no admission budget.
- New EGC knobs (`maxTtl`/`staleTtl`/`negativeTtl`) were rejected: they could not govern the egress
  instance and no tuning need is reported. `cacheTtl` and `outbound_egress_dns_cache_ttl_ms` are
  fallback TTLs.
- A predispatch connect failure still marks the whole AI backend tried; same-backend re-dial on
  another address and ExternalName in-request address failover are not implemented (rotation
  limits a dead address to its share of first attempts).
- The per-key flight mutex is held across the DNS round trip under the recorded V-1 exemption
  for data-free single-flight mutexes (decided by the user on 2026-11-06).

## Verification and re-evaluation

Verified 2026-11-06: `edgion-common` `dns::cache::tests` (fresh/key separation, TTL clamp and
fallback cap, exclusive deadlines, disable/literal/long-name bypass, 20-way coalescing, queued
waiter handover after leader cancellation, stale-while-revalidate, transient-failure back-off,
`NoAddresses` drop, negative window and error identity, late failure never overwriting a newer
answer — mutation-checked), `egress::tests`
(raw answer classified per resolver and per use), `dns::tests` (borrowed fingerprints, readiness),
`httpdns` `ttlPath` tests, the `httpdns_fetcher` UDP test (reported 30 s record TTL), and Gateway
`dns_cache_test::warm_hit_allocates_only_the_owned_address_result` plus the ExternalName dispatch
and AI selection tests.

Re-evaluate when: a resolution input outside fingerprint/name/policy appears; profiles show
contention on one hot key; operators need tunable stale/negative windows; the System resolver path
gains a way to distinguish NXDOMAIN; or AI connect-failure handling gains same-backend re-dial.

## Resolution references

- `common-02` — 2026-11-06, fixed (shared TTL-honoring answer cache, coalescing, stale and
  negative entries, AI rotation).
- `task-externalname-dns-cache-identity-and-lifecycle` (origin D0001) — 2026-09-03, fixed; superseded
  in structure by `common-02`.
- Retired decision: [ExternalName miss coalescing not implemented](../superseded/externalname-dns-miss-coalescing-not-implemented.md).
