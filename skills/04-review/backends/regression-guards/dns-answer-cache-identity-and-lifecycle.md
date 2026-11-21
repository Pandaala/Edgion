---
name: dns-answer-cache-identity-and-lifecycle
description: Use when reviewing DNS answer caching for AI upstreams, external jumps or EgressResolver; covers the shared DnsAnswerCache key, TTL/stale/negative lifecycle, coalescing, bounds and consumer contracts.
status: fixed
finding_id: common-02 (supersedes task-externalname-dns-cache-identity-and-lifecycle, origin D0001)
closed: 2026-11-06
knowledge_type: regression-guards
---

# DNS Answer Cache Identity and Lifecycle (common-02; origin D0001) — FIXED

## Conclusion and scope

All DNS answer caching goes through one canonical mechanism,
`edgion_common::dns::DnsAnswerCache` (`edgion-common/src/dns/cache.rs`). It fronts
`resolve_answer` for three process-wide instances:

| Instance | Consumers | Sizing |
|---|---|---|
| `egress_answer_cache()` (`edgion-common`) | `EgressResolver`s with operator-configured destinations (Gateway plugins, LinkSys, Wasm callouts with allowlisted origins, GeoIP, Controller OAuth2) | 4096 entries, 16 shards |
| `request_egress_answer_cache()` (`edgion-gateway/src/plugins/http/common/outbound/egress.rs`) | `EgressResolver::new_request_influenced`: DSL `http.call` and replayable redirects when `httpBlockPrivateIps` is enabled or the Gateway blocked CIDR list is nonempty | 1024 entries, 8 shards |
| `upstream_dns_cache()` (`edgion-gateway/src/backends/discovery/dns_cache.rs`) | AI provider endpoints, ExternalJump destinations of RegionRoute / DynamicExternalUpstream (`util::net::resolve_domain`), and new UDP sessions to `system`-mode ExternalName Services (`backends::prepare_l4_service`, 250 ms budget, single-flight so spoofable sources cannot multiply queries) | `lru_cache` defaults with an 8-shard floor |

Callers must not add another DNS cache type; a new instance is justified only to isolate a
consumer class. The instances are separate so names a request or upstream can choose cannot
evict the answers of upstream or operator-configured egress consumers. Consumers built with a
zero fallback TTL bypass caching entirely (the HTTPDNS bootstrap fetcher, OTLP transport).
Deliberately uncached resolution: the Controller DNS resolver service and the Gateway DNS
endpoint table (both resolve in the background and publish EndpointSlices; see
[endpoint table lifecycle](dns-endpoint-table-lifecycle.md)), ExternalName `system` mode on
HTTP, gRPC, TCP and TLS (OS resolver per attempt). With DSL private blocking off and an empty Gateway CIDR list,
the checked system resolver bypasses Edgion's DNS answer cache. It filters the returned
socket addresses and passes each admitted result to reqwest with its original IPv6
interface scope ID. The cached path stores IPs only and does not promise to retain
an OS scope ID. Both paths classify the dial candidates. The shared answer cache caps
answers at 256 IPs. The uncached DSL resolver checks the first 256 distinct sockets
(`MAX_ANSWER_ADDRS`), in OS order. The Gateway resolver-selection test starts an isolated
process with an empty CIDR list and checks the scope ID returned for a numeric scoped IPv6 name.

**Key** = `(ResolverSpec::fingerprint(), queried name, DnsQueryPolicy)`. These inputs describe the
answer completely: a changed hostname, resolver or family is a different key, so updates need no
invalidation and deleted Services own nothing (the key carries no Service identity). HTTPDNS
fingerprints include the generation's readiness (`httpdns:ready:` / `httpdns:fallback:`), so a
fallback-only generation never reads a ready generation's answers, and the owning LinkSys
`namespace/name`, so two tenants with identical (redacted) config text but different CA or
client-certificate Secrets never share answers, failures or refresh state; secret-only rotation
keeps the fingerprint. StandardDns fingerprints are precomputed in `StandardDnsServers` at configuration
time from the parsed `SocketAddr`s the resolver queries (never the raw strings, so `["a,b"]`
cannot collide with `["a","b"]`; a list without one valid server is `std:!invalid`), so a warm hit
allocates nothing for the key. EGC ingest rejects unparsable `dnsResolver.servers` entries.

**Lifecycle** (deadlines stamped at write time from the writer's `DnsCachePolicy`):

- Fresh for the reported record TTL clamped to `[5 s, 300 s]` (floor = Envoy
  `dns_min_refresh_rate`), or for the consumer's fallback TTL (also capped at 300 s) when the
  resolver reports none (System resolver, HTTPDNS without `ttlPath`). Fallback TTL `0` disables
  the cache for that consumer. IP literals and names longer than 254 bytes always bypass it.
- Stale for 30 s afterwards: the first request to claim the entry's `refreshing` flag refreshes
  inline; every other request gets the stale answer without waiting or touching the flight
  registry (readers check the flag before claiming it). A claim on an entry that was replaced is
  never released, so a reader holding the retired entry cannot start a second refresh. A refresh
  cancelled by its caller (an outbound connect timeout shorter than a hung resolver) counts as a
  transient failure of the claimed answer when it is still current (or was evicted) and within
  its stale limit — kept, window slid, refresh backed off — otherwise the claim is only released;
  a cancellation never writes a negative entry.
- DNS failure keeps the last good answer (Envoy keeps its last resolution): a transient refresh
  failure keeps serving the stale answer and slides its stale window to `now + 30 s`, so a
  still-requested answer survives an outage while an unused one expires. Every answer has a hard
  **stale limit** of `fresh_until + 300 s` (RFC 8767 maximum stale timer, shortened because the
  gateway keeps dialing the answer): sliding stops there and a failure past it drops the answer,
  so a deleted or reassigned destination stops receiving traffic within TTL + 5 min. A refresh
  claimed while the answer was servable keeps it even if the resolver times out after the window
  ended (also when the entry was evicted meanwhile: the claimed answer is stored again), and a
  lookup (fast or cold path) arriving after the window while that refresh runs is served the
  answer instead of resolving a second time (`AnswerEntry::is_servable`). Answers are retained in
  the LRU until their stale limit plus one stale window, because a refresh can outlive the 30 s
  window (HTTPDNS `timeoutMs` up to 120 s plus its fallback). Refreshes
  back off 1, 2, 4, 8, then 10 s (cap = Envoy's default 10x base). `NoAddresses` (authoritative
  empty answer, including Standard DNS NXDOMAIN) drops the answer. System-resolver NXDOMAIN, and
  on the `Family` policy (AI, ExternalName) Standard DNS empty answers without SOA evidence,
  surface as `Io`, so for them a removed name keeps being served while it is requested and the
  resolver keeps failing, up to the stale limit. The `Native` policy (egress consumers,
  including the Controller OAuth2 client) maps every Standard DNS `NoRecordsFound` to
  `NoAddresses`; the uncached Controller DNS resolver service uses `Family`.
- Write rule: a failure never overwrites a newer answer. When a lookup fails, it reads the current
  entry; an answer written by someone else that is still servable (inside its stale window, or
  under its own running refresh) wins and is served unchanged.
- Cold miss: same-key callers serialize on a per-key flight mutex from the shared
  `edgion_common::singleflight::FlightRegistry` and re-check the cache after acquiring it. A
  cancelled leader releases the mutex and the next waiter takes over. The mutex guards no data
  and only serializes same-key misses — the JWKS-provider shape (exempt from design rule V-1, see
  [data-free single-flight mutex](../../concurrency/intentional-design/single-flight-mutex-across-io.md)).
- A failure with no servable fallback answer is a negative entry for the current back-off
  (1 s, doubling per consecutive failure up to 10 s); the failure count outlives the window so
  the back-off can grow.

**Address selection**: cached answers keep their order for the TTL, so ExternalJump
(`rotate_addresses`; ExternalJump dials the first address passing the
SSRF checks, `first_safe_resolved_addr`) and AI attempts (`select_ai_addr`) start at the shared process-wide
`next_address_rotation()` offset (`edgion-gateway/src/backends/mod.rs`); AI attempts also prefer
addresses the request has not dialed, until the 8-slot `DialedAddrs` record overflows, after which
rotation alone selects. `system`-mode ExternalName lookups (uncached) also rotate the OS
order from that offset. Exact numeric bounds live in the `edgion-common/src/dns/cache.rs` module docs and the
operator page `docs/en/ops-guide/configuration/edgion-gateway-config.md`.

**Consumer contract**: the cache stores raw answers. Every consumer applies its own egress or
upstream address classification on each use (`filter_resolved_addrs*`), so one answer can serve
consumers with different `blockPrivate` policies and the dialed address is always classified.
`edgion_outbound_egress_blocked_total` therefore counts per use.

**Deadlines and observability**: every resolver call inside a flight is bounded (hickory query
timeouts, the HTTPDNS request timeout, 30 s of waiting for the System resolver).
Cancellation or timeout aborts queued system lookups on the blocking pool. A lookup
already executing `getaddrinfo` finishes on its worker thread. The stale refresh runs
inline on the claiming request, so during an outage one request per key per back-off interval
waits up to that deadline (accepted; a background refresher was rejected). Store-stats list
in-flight keys as `policy|name` only; the request-influenced instance reports counts only.
Per-lookup hickory resolvers are built with their own response cache disabled.

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
  instance and no tuning need is reported. EGC `fallbackRefreshSeconds` (formerly `cacheTtl`)
  and `outbound_egress_dns_cache_ttl_ms` are fallback TTLs.
- A predispatch connect failure still marks the whole AI backend tried; same-backend re-dial on
  another address is not implemented (rotation limits a dead address to its share of first
  attempts). ExternalName backends no longer use this cache, except new UDP sessions in
  `system` mode: `gateway`/`controller` addresses are ordinary endpoints with LB, health check
  and outlier ejection (2026-11-10 DNS resolution redesign).
- Accepted costs (second post-merge review, 2026-11-07): each entry stores its fingerprint twice
  (map key and LRU node; ~1-2.5 KB with HTTPDNS, capacity-bounded); queued cold waiters are
  released one by one by the FIFO flight mutex (visible only on a cold key at very high QPS);
  abandoned `getaddrinfo` threads finish on the blocking pool.
- Accepted limitation (2026-11-07 fix-round review): when a refresh's claimed answer was evicted,
  a transient failure stores it again even if another lookup wrote and lost a newer result (an
  answer or `NoAddresses`) for the key meanwhile. It needs LRU pressure plus a refresh longer than
  that write's retention (~11 s for a negative entry); the stale limit still bounds the
  resurrected answer. Detecting it needs per-key write generations that survive eviction.
- The per-key flight mutex is held across the DNS round trip under the recorded V-1 exemption
  for data-free single-flight mutexes (decided by the user on 2026-11-06).

## Verification and re-evaluation

The common-01 scope regression test feeds fabricated system socket results to
`UncachedLocalSystemResolver`. It checks filtering, deduplication, the answer cap
and distinct interface IDs on the same IPv6 address. It exercises the filter
boundary without requiring a particular OS DNS response or a live link-local dial.

Re-verified 2026-11-07 after a second five-reviewer review (spec §10 "Second post-merge
review"): tests for cancelled refreshes counted as failures (single and repeated, to the stale
limit), a newer answer under its own refresh surviving a failure, a refresh claimed before and
failing after the limit with the back-off carried on, listed flight keys, the retention bound,
the owner-namespace HTTPDNS fingerprint, family dedup, ExternalJump first-safe-address
selection and literal handling, and an AI dialed-skip test over a seeded cache (mutation-checked
where noted in the spec). The `SYSTEM_RESOLVE_TIMEOUT` bound, wall-clock LRU expiry and the
256-address truncation are covered by inspection only.

Re-verified 2026-11-07 after a five-reviewer post-merge review (dispositions in the spec,
§10 "Post-merge review"): new `dns::cache::tests` for the stale limit, cold lookups during a
running refresh and refresh failure after eviction (both mutation-checked), failure coalescing,
stale-refresh cancellation, capacity and retention; `dns::tests` parsed-address fingerprints;
EGC `servers` validation; Gateway ExternalName rotation wiring and AI dialed-skip / overflow tests.

Verified 2026-11-06: `edgion-common` `dns::cache::tests` (fresh/key separation, TTL clamp and
fallback cap, exclusive deadlines, disable/literal/long-name bypass, 20-way coalescing, queued
waiter handover after leader cancellation, stale-while-revalidate, transient-failure back-off,
`NoAddresses` drop, negative window and error identity, late failure never overwriting a newer
answer — mutation-checked), `egress::tests`
(raw answer classified per resolver and per use), `dns::tests` (borrowed fingerprints, readiness),
`httpdns` `ttlPath` tests, the `httpdns_fetcher` UDP test (reported 30 s record TTL), and Gateway
`dns_cache_test::warm_hit_allocates_only_the_owned_address_result` plus the ExternalName dispatch
and AI selection tests.

Re-evaluate when: a resolution input outside fingerprint/name/policy appears; operators need a
different stale limit or an explicit purge; profiles show
contention on one hot key; operators need tunable stale/negative windows; the System resolver path
gains a way to distinguish NXDOMAIN; or AI connect-failure handling gains same-backend re-dial.

## Resolution references

- `common-02` — 2026-11-06, fixed (shared TTL-honoring answer cache, coalescing, stale and
  negative entries, AI rotation); 2026-11-07 post-merge review fixes (stale limit, refresh races,
  parsed-address fingerprint, request-influenced DSL instance).
- `task-externalname-dns-cache-identity-and-lifecycle` (origin D0001) — 2026-09-03, fixed; superseded
  in structure by `common-02`.
- Retired decision: [ExternalName miss coalescing not implemented](../superseded/externalname-dns-miss-coalescing-not-implemented.md).
