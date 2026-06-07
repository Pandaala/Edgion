# lb-01: Panic threshold for load balancing (Envoy-style all-unhealthy fallback)

- **Severity**: High
- **Dimension**: Load balancing / resilience — data-plane availability
- **Status**: promoted from review finding (`tasks/review-envoy/lb-01-no-panic-threshold.md`, deleted on promotion)
- **Promoted**: 2026-05-29 (Scope Gate Option A — wide-impact feature, not a narrow review fix)
- **Needs**: brainstorming workflow before implementation (config-surface design is the open question)

## Why this is a task, not a review fix

The original finding's own "Suggested direction" describes a **new configurable LB
mechanism mirroring Envoy's panic threshold**, not a localized bug fix. It triggers all
three Scope-Gate wide-impact conditions:

1. Touches a core abstraction — the `CompiledSelector::select` / health-filter contract.
2. Affects behavior across all four LB policies (RoundRobin / ConsistentHash / LeastConn /
   EWMA) and every transport that selects backends (HTTP, TCP, UDP, TLS routes, mirror plugin).
3. Requires coordinated changes across layers — new config knob (global gateway config
   and/or per-service annotation) + gateway selection path + tests.

## Envoy's design (the proven baseline)

Envoy implements a **panic threshold** for outlier detection. When the percentage of healthy
hosts in a cluster drops below the configured threshold (default 50%), Envoy disables outlier
ejection and routes traffic to all hosts regardless of health state. Rationale: if more than
half the cluster is marked unhealthy, the health signals more likely reflect a systemic issue
(network partition, misconfigured probe) than genuine backend failures. Routing to all hosts
avoids concentrating traffic on the few remaining "healthy" ones (which may themselves be
overloaded) and prevents a cascade where the last healthy host also fails. Envoy applies the
same logic to passive health checking: if `max_ejection_percent` would eject so many hosts
that fewer than `panic_threshold` remain, it stops ejecting.

Reference: Envoy docs — Panic Thresholds; `cluster.panic_threshold` (default 50%).

## Edgion's current design (the gap)

`is_backend_healthy` (`src/core/gateway/backends/selector.rs:37-55`) applies the active
health-check filter and the circuit-breaker ejection filter as an unconditional two-gate
predicate. When every backend fails the predicate, `CompiledSelector::select`
(`src/core/gateway/lb/compiled_selector.rs:207-235`) exhausts `SELECT_MAX_ITERATIONS` (256)
and returns `None`; `pick` (`selector.rs:201-205`) maps that to a `not_found` status
(e.g. `BackendEndpointSliceNotFoundByRoundRobin`) and the request fails.

The circuit-breaker `max_ejection_percent` (default 50%, `circuit_breaker/store.rs`) caps
how many backends the breaker ejects, but:

1. **No panic-threshold counterpart for active health checks**: if `unhealthy_threshold`
   probes fail for every backend, all enter `unhealthy_backends` and every subsequent
   request is dropped — no traffic reaches any backend.
2. The two mechanisms (active HC + circuit breaker) have **independent caps**. All
   active-HC-healthy backends can still be ejected by the breaker, and
   `max_ejection_percent` is computed against the selector-snapshot total, not the healthy
   subset — so the surviving "healthy" count is not the enforced floor.
3. There is **no combined** "if healthy fraction < X, ignore all health signals" override.

## Impact

- **Full service outage** when active health checks mark all backends unhealthy, regardless
  of actual service availability.
- **Silent degradation**: operators see 503/502 from the gateway; backends may be perfectly
  healthy but unreachable from the gateway's LB perspective.
- A single misconfigured health-check path (`/healthz` → `/healthCheck` rename) can take the
  entire gateway offline silently in `O(unhealthy_threshold × interval)` seconds.

## Suggested direction (to be refined in brainstorming)

Implement a panic threshold in the selection path. Before applying the health filter, count
how many backends would pass it; if the healthy fraction is below a configurable threshold
(e.g. 50%), bypass the health filter and select from all backends as if none were filtered.
Both the active-HC and circuit-breaker gates should participate in the "how many would
survive" calculation. This mirrors Envoy's panic threshold.

## Open design questions for brainstorming

1. **Where to count.** `CompiledSelector::select` currently walks candidates lazily via the
   inner selector + `health_filter` closure; it never materializes the "how many pass"
   count. A panic decision needs the healthy/total ratio *before or during* selection.
   Options: precompute healthy count per request (extra O(n) pass), or track a healthy
   counter alongside the health/CB stores. Must stay off the per-request hot path where
   possible (`SELECT_MAX_ITERATIONS` already bounds the walk).
2. **Config surface.** Global gateway config knob (`EdgionGatewayConfig`) vs. per-service
   annotation (`edgion.io/*`) vs. both, with precedence. Default value (Envoy uses 50%;
   `0` would disable the feature for opt-out).
3. **Interaction with circuit breaker `max_ejection_percent`.** Should the panic threshold
   subsume / coordinate with the breaker's own cap, or remain an independent final safety
   valve? Avoid double-counting the two gates.
4. **Observability.** When panic mode engages, emit a metric / log (control-plane side, not
   the data-plane hot path) so operators can see "panic mode active for service X".
   Respect Iron Rule 3 — no `tracing::` on the request path.
5. **Per-policy consistency.** The fallback must behave identically across RoundRobin,
   ConsistentHash (including the empty-key RR fallback), LeastConn, and EWMA, so the four
   `CompiledSelector::select` arms cannot drift.
6. **Status-code semantics.** When panic mode routes to an all-unhealthy pool, what does the
   request observe on a genuinely-dead backend (connect failure → existing retry/error
   path)? Confirm this is strictly better than the current hard `not_found`.

## References

- `src/core/gateway/backends/selector.rs` — `is_backend_healthy`, `pick`, `select_*`
- `src/core/gateway/lb/compiled_selector.rs` — `CompiledSelector::select`, `RrCore`/`ChCore`/`MetricCore`
- `src/core/gateway/backends/circuit_breaker/store.rs` — `max_ejection_percent`
- `src/core/gateway/backends/health/` — active health check status store
- Skills: `skills/01-architecture/02-gateway/` (LB selection), `skills/04-review/lb/`
