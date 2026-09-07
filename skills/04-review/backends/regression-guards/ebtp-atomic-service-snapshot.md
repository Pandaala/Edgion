---
name: ebtp-atomic-service-snapshot
description: Use when reviewing EdgionBackendTrafficPolicy Service request reads; request-visible fields must come from one captured generation, not live DashMap or store getters.
status: fixed
finding_id: ebtp-atomic-service-snapshot
closed: 2026-09-05
knowledge_type: regression-guards
---

# ebtp-atomic-service-snapshot — one captured Service policy generation — FIXED

## Conclusion

HTTP/gRPC Service requests capture one `Arc<EffectiveBackendTrafficPolicy>` at
first `begin_upstream_attempt` and reuse it for later attempts of that request.
Live `get_lb_policy` / `get_authority` / `get_retry_budget` /
`get_circuit_breaker_limit` / `get_connect_timeout` / `resolve_lb_policy` /
`resolve_degrade_threshold` / `resolve_connect_deadline` reads on that proxy path
are **not accepted**. Restoring request-visible DashMaps is **not accepted**.

## Core Rationale

**1. Request-visible fields are one generation**

Load balancer, compiled authority, panic override, retry-budget Arc, circuit-breaker
limit, and connect timeout publish together on the typed snapshot. A Pingora retry
that re-enters `upstream_peer` must not mix those fields from two publications.

**2. Capture is per request, not per TCP keepalive**

A new request on a reused connection loads the current snapshot. A retry of the
same request keeps the already captured Arc. Live counts (`CbPermit`, LeastConn
inflight, `RetryBudget` counters, outlier ejections) stay on those Arcs and are
not copied into the generation.

**3. A missing capture is “no policy”**

Service/gRPC readers must not fall back to the live store when
`BackendContext.effective_policy` is `None`. gRPC capture must not require
`selected_backend`. AI continues to capture at plan time onto `AiHttpState`.

## Fix Suggestions Not Accepted

- Restore live `get_lb_policy` / `resolve_lb_policy` on the HTTP/gRPC `try_get_peer`
  path — reopens mixed-generation LB versus timeout/authority/retry.
- Keep `resolve_connect_deadline` → `get_connect_timeout` as the Service timeout
  source after capture exists — a half-migration.
- Let HTTP `pick` call `resolve_degrade_threshold` internally — the captured
  override never pins panic for that request.
- Gate Service capture on `selected_backend.is_some()` — gRPC has only
  `service_key` and would keep a live fallback.
- Freeze health/outlier **status**, inflight counts, or endpoint lists into the
  snapshot — those are live runtime, not configuration.
- Swap `RetryBudget` / logical-CB / AI outlier Arcs on equivalent conf_sync
  replay — live counters must not change object identity on a no-op resync.
- Treat L4/`request_mirror` panic-store reads or `request_mirror` Host-as-address
  as this finding — those are documented residuals, not HTTP/gRPC mixed-generation
  defects.

## Re-evaluation Triggers

Re-open this decision only if:

- L4 or `request_mirror` grow a `BackendContext` and need the same capture
  contract (separate task; do not silently change L4 LB away from RoundRobin).
- A new Service request reader is added that loads policy configuration after
  `begin_upstream_attempt` without using the captured Arc.

## Reference Cases

- `tasks/todo/backend-traffic-policy-atomic-service-snapshot.md`
- `edgion-gateway/src/backends/policy/backend_traffic/store.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`
- Sibling todos remain out of scope: retry-budget window rotation, LB runtime-handle
  capture, degrade-mode gauge lifecycle.

## Resolution references

- `ebtp-atomic-service-snapshot` — 2026-09-05.
