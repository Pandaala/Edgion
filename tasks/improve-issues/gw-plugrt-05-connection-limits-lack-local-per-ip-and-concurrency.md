# gw-plugrt-05: L4 per-source-IP connection limiting exists only as a Redis call on the serial accept path; no local per-IP or concurrent-connection limit

- **Category**: best-practice
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs:155-245` (`redis_dimension`, `on_connection`); `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs:93-99` (`StreamPlugin` has only `on_connection`); `edgion-gateway/src/plugins/stream/connection_filter_bridge.rs:43-71` (Stage 1 `should_accept`)

## Summary
`ConnectionRateLimit.perSourceIp` is implemented only as a Redis round trip per accepted connection
(bounded by `redisTimeout`, default 100 ms, cap 1 s) with default `onRedisFailure: Allow`. There is no local
per-source-IP bucket and no concurrent-connection cap at all (the stream plugin trait has no close hook).
The most common L4 abuse control — "at most N connections / N new connections per second from one IP" —
therefore always depends on Redis latency on the serialized accept path and turns off by default when Redis
is slow, i.e. under the flood it is meant to stop.

## Current behavior (evidence)
- `on_connection` (`plugin.rs:198-245`): local token bucket only for `perListener`; `perSourceIp` calls
  `redis_dimension` unconditionally (`:228-243`).
- `redis_dimension` (`:155-193`) awaits `tokio::time::timeout(self.redis_timeout, ...)`; error/timeout
  returns `matches!(on_failure, OnRedisFailure::Allow)` — default `Allow` for `perSourceIp`.
- Stage 1 admission runs inside Pingora `ConnectionFilter::should_accept` before the handshake task is
  spawned (`connection_filter_bridge.rs:43-71`); per
  `skills/04-review/listeners/intentional-design/bounded-serial-connection-admission.md` one accept task holds
  one socket in filtering at a time, so each per-IP Redis RTT directly caps accept throughput.
- `StreamPlugin` exposes only `on_connection` (`stream_plugin_trait.rs:93-99`), so no plugin can track
  live connections and enforce a concurrency limit.

## Industry comparison
- NGINX `limit_conn_zone $binary_remote_addr` + `limit_conn` (concurrent per IP) and `limit_req` are local
  shared-memory limits with no external dependency.
- HAProxy stick tables (`src_conn_cur`, `src_conn_rate`) track per-IP concurrency and rate locally, with
  optional peer sync rather than a synchronous remote call per connection.
- Envoy `envoy.filters.network.connection_limit` caps concurrent connections locally; the network
  `ratelimit` filter (remote) is an add-on, not the only per-descriptor mechanism.

## Impact
- A single client opening connections in a loop can only be throttled with Redis in the path; under
  attack, Redis load scales with the attack rate, timeouts push the default policy to Allow, and accept
  throughput is bounded by Redis RTT for legitimate clients too.
- Slowloris-style connection hoarding from one IP cannot be bounded (no concurrency limit).

## Suggested direction
- Add a local `perSourceIp` mode: a bounded sharded table (`edgion_algo::cache::ShardedLruCache` or a
  count-min sketch like `RateLimitLocal`) keyed by canonical `DirectPeerIp`, evaluated before any Redis
  dimension; keep Redis as the optional distributed layer (mirroring `perListener` + `redisDistributed`).
- Add an optional `maxConnectionsPerSourceIp` / per-listener concurrency cap by giving `StreamPlugin` an
  admission guard object whose `Drop` decrements the counter when the connection closes.

## Notes
- `bounded-serial-connection-admission.md` accepts accept serialization and recommends local `perListener`
  for burst protection; it does not discuss the absence of any local per-IP or concurrency mechanism, which
  is the gap filed here. `rate-limit/intentional-design/redis-fail-open.md` accepts the fail-open default; this
  finding does not propose changing it — it removes the need to depend on Redis for the common case.
  Not tracked under `tasks/`.

## Related findings

- arch-02 (process-level connection admission / overload manager).

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs:198-245`: local gate only for `perListener`; `perSourceIp` always calls `redis_dimension`.
- `edgion-resources/src/resources/edgion_stream_plugins/connection_rate_limit_config.rs:60-69`: `PerSourceIpLimit.on_redis_failure` defaults to Allow.
- `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs:94-100`: `StreamPlugin` has only `on_connection`, so no plugin can maintain a live-connection count; no per-IP/max-connections setting exists elsewhere in `edgion-resources/src` or `edgion-gateway/src`.
- `skills/04-review/listeners/intentional-design/bounded-serial-connection-admission.md` and `rate-limit/intentional-design/redis-fail-open.md` accept serialization and fail-open, not the absence of a local per-IP/concurrency limiter.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `connection_rate_limit/plugin.rs`, `stream_plugin_trait.rs` and `connection_rate_limit_config.rs` have no diff; `local_gate.rs` change is a `try_update` rename only.
