# arch-02: No process-level overload protection (connection admission / memory-pressure shedding)

- **Category**: best-practice
- **Severity**: P2 (downgraded from P1 by independent verification)
- **Scope**: large-direction
- **Location**:
  - `edgion-gateway/src/runtime/server/listener_builder.rs:585-600` (listener accept path: only PROXY-protocol guard + stream-plugin `ConnectionFilter`)
  - `edgion-gateway/src/plugins/stream/connection_rate_limit/` (per-second rate, not a concurrency cap)
  - `edgion-gateway/src/runtime/server/downstream_h2.rs:32` (only per-connection H2 stream cap)
  - Pingora fork `pingora-core` (no `max_connections` concept in the listener/accept loop)

## Summary

The Gateway has many *local* bounds (request-body budget, response-cache bytes, AI conversion budget,
per-connection H2 streams), but nothing that protects the *process*: there is no global or per-listener cap
on concurrently open downstream connections, no cap on in-flight requests, and no reaction to memory
pressure. Under a connection flood or slow-client attack the process accepts until it hits `NOFILE` or the
cgroup OOM killer — which kills every in-flight request on the pod at once.

## Current behavior (evidence)

- Accept path: `listener_builder.rs` installs `GuardFirstConnectionFilter` (PROXY protocol trust) and an
  optional `StreamPluginConnectionFilter`; both are policy filters, not admission control.
- `connection_rate_limit` limits new connections per time window per key; it does not bound concurrent
  connections, so long-lived idle/slow connections (slowloris, idle H2, WebSocket, SSE) accumulate freely.
- `downstream_h2_max_concurrent_streams` (1..=1024) bounds streams per connection only; N connections × 1024
  streams is unbounded.
- The upstream Pingora fork has no `max_connections` accept gate (verified in the locked checkout).
- `skills/04-review/listeners/intentional-design/linux-host-tuning-baseline.md` explicitly states the
  `somaxconn`/`NOFILE` guidance "is deployment guidance, not an active-connection admission guarantee" —
  the gap is acknowledged but not addressed.

## Industry comparison

- **Envoy**: Overload Manager with resource monitors (heap size, active downstream connections via
  `global_downstream_max_connections`) and actions (`stop_accepting_connections`,
  `disable_http_keepalive`, `stop_accepting_requests`, `reduce_timeouts`, `shrink_heap`); per-listener
  `max_connections` via the connection-limit filter.
- **NGINX**: `worker_connections` hard cap; `limit_conn` for per-key concurrency.
- **HAProxy**: `maxconn` (global, per frontend, per server) with queueing.
- **Traefik/Kong**: inherit Go `net/http` / NGINX limits; Kong documents `nginx_events_worker_connections`.

## Impact

- A single tenant or attacker can exhaust file descriptors / memory and take down all Gateways/listeners in
  the process (blast radius = the pod), instead of being shed with 503/close at the edge of capacity.
- OOM-kill is the only "shedding", causing correlated failures across replicas under the same flood.
- Slow-client attacks are mitigated only by timeouts, which are themselves fixed rather than tightened
  under pressure.

## Suggested direction

1. Add a process-wide **active downstream connection cap** (and optional per-listener caps) enforced at
   accept time with an RAII guard (counter decremented on connection drop) — immediate close when over cap,
   counted in one bounded metric. This can live in the existing `GuardFirstConnectionFilter` seam, no
   Pingora change needed for the check itself.
2. Add a small **overload controller**: periodically sample allocator stats (jemalloc `stats.allocated`,
   already the default allocator) against a configured budget, and at thresholds (a) disable keep-alive,
   (b) reject new connections, (c) reject new requests with 503 — mirroring Envoy's action ladder. Keep it a
   restart-only gateway-local setting (like `request_body_budget`).
3. Optionally a per-key concurrent-connection limiter (`limit_conn`) as a mode of `connection_rate_limit`
   rather than a new plugin.

## Notes

- Not a duplicate of the request-body budget (`request_body/budget`), which only bounds body storage.
- Searched `skills/04-review/{listeners,cpu-memory}` and `tasks/`: no accepted design rejects connection
  admission; the host-tuning page explicitly leaves it open.

## Related findings

- gw-plugrt-05 (no local per-IP / concurrent-connection limiting in stream plugins).

## Verification (independent)

- **Verdict**: DOWNGRADED
- **Severity**: P2 (was P1)
- `grep -rliE "max_?connections|active_conn|overload" edgion-gateway/src` finds no process- or listener-level connection cap; matches are unrelated (LB, AI quota, ES bulk).
- Locked Pingora fork checkout `e5f4287` `pingora-core/src`: no `max_conn`/accept semaphore.
- `skills/04-review/listeners/intentional-design/bounded-serial-connection-admission.md` covers connection *rate* limiting/accept serialization only; `linux-host-tuning-baseline.md:15` states host tuning is not an admission guarantee.
- Downgraded to P2: no defect in shipped behavior; Envoy without an explicitly configured overload manager/`global_downstream_max_connections` has the same exposure, and timeouts plus per-listener connection rate limiting exist. Still a sensible production-hardening item.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `edgion-gateway/src/runtime/server/listener_builder.rs:585-600` and `downstream_h2.rs:32` unchanged since `feature-08-30`; the accept path still installs only `GuardFirstConnectionFilter` and the optional `StreamPluginConnectionFilter`.
- `grep -rliE "max_?connections|active_conn|overload|connection_cap" edgion-gateway/src edgion-resources/src` still finds no process/listener connection cap (only LB, AI quota, ES bulk, request-body contract matches).
- Only `connection_rate_limit/local_gate.rs` changed (1 line); it remains a rate, not a concurrency cap. Fork moved to `edgion_v0.9.1@e5f4287` with no accept-gate added.
