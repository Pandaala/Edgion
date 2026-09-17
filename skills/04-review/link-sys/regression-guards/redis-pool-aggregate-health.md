---
name: redis-pool-aggregate-health
description: Use when reviewing Redis LinkSys pool health, Fred connection callbacks, or Redis connection-event logging.
knowledge_type: regression-guards
finding_id: redis-pool-health-logging
---

# Redis pool health is aggregate and connection logs are transition-only

## Conclusion and scope

`RedisLinkClient::healthy` means that at least one Fred pool member is connected.
The background aggregate probe reads `ClientLike::is_connected()` from every fixed
pool member and degrades the provider only when no member remains usable. This
matches Fred's default `prefer_connected` routing, which selects a connected member
when one exists.

Redis connection callbacks are not used for Edgion health or logging because their
events describe individual server connections inside a Fred client and can be
delivered by independent tasks. The aggregate probe feeds the shared LinkSys
transition path, so INFO/WARN records contain only the validated LinkSys name and
fixed provider type and are emitted only when aggregate health changes.

## Rationale and evidence

The previous implementation registered callbacks on every `Pool::clients()` member
but let all of them unconditionally overwrite one `Arc<AtomicBool>`. The most recent
member event therefore represented the whole pool: one error could reject work even
while another member remained connected, and one reconnect could hide that every
other member was down. The same callbacks logged the Fred `Server` and error once per
member event.

`RedisLinkClient` now starts the canonical `health_probe` driver after Fred
initialization. Its provider closure derives `any_connected` directly from Fred's
current client states at a nominal one-second healthy cadence; request-side
`healthy()` remains one lock-free atomic read. `mark_retired` sets the retired flag
and signals the probe watch without aborting the task; shutdown then joins the probe
before quitting the pool, preventing an in-flight result from restoring health after
the terminal false state. A retired probe must not write `edgion_link_sys_health`.
Shutdown itself does not write the shared health gauge. During a same-key replacement,
live probes of the published generation still refresh the gauge from the current
observation so the active generation can self-heal after overlap. Flip counters and
logs remain CAS-transition-only. A deletion can therefore leave the already accepted
stale per-link series.
`RedisClientConfig.pool.size`, Fred pool construction, command retry semantics, and
active admin `PING` health checks are unchanged.

The applicable authorities are `skills/00-design-rules.md` sections A through C and
`skills/03-coding/01-log-safety.md`: backend topology must not be exposed in INFO
logs, and event observability must be bounded. The existing LinkSys health transition
helper supplies transition-gated flip metrics and rate-limited metadata-only logs.

## Alternatives and limits

Computing `Pool::clients().any(ClientLike::is_connected)` in every `healthy()` call
would avoid a cached state but would turn a request-path atomic read into an O(pool
size) scan. Mirroring `on_error` and `on_reconnect` into per-member bits is also
incorrect: one Fred client can own multiple Cluster or Sentinel connections, and the
two event streams have no cross-listener ordering guarantee. Treating health as "all
connected" would unnecessarily reject work that Fred can route to a connected
member.

The cached state is sampled at a nominal one-second healthy cadence; scheduling and
jitter mean this is not a hard detection bound. Unhealthy probes retry with the shared
bounded backoff. It proves neither that every Redis node or cluster hash slot is
reachable nor that the next command will succeed. Command errors remain authoritative
for an individual operation. `health_status()` separately performs a live `PING` and
is not defined by this cached flag.

## Verification and re-evaluation

Unit tests cover the any-connected predicate and an uninitialized real Fred pool.
The shared probe driver separately covers transition deduplication, backoff, and
shutdown signaling. Re-evaluate this rule when upgrading Fred's client-state
semantics, changing `prefer_connected`, replacing the fixed pool, or changing request
consumers so `healthy()` no longer gates Redis operations.

The shared metric's deletion-time staleness and same-key generation ownership follow
the accepted LinkSys health-metric decision in
`../intentional-design/health-metric-free-function-and-cardinality.md`.

## Resolution references

- `redis-pool-health-logging` — corrected the last-event-wins pool state and
  per-member endpoint logging in `RedisLinkClient`.
