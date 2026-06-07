# LB Panic Threshold — Design Spec

- **Date**: 2026-06-07
- **Task**: `tasks/todo/lb-01-no-panic-threshold.md`
- **Severity**: High (data-plane availability)
- **Status**: design approved, pending implementation plan

## Problem

When active health checks and/or the circuit breaker mark **every** backend of a
service as unhealthy, `CompiledSelector::select` exhausts its fallback iterations and
returns `None`, which `pick` maps to a `not_found` status — the gateway returns 503 for
that service even when the backends are actually serving traffic.

A single misconfigured probe (e.g. renaming `/healthz` → `/health` in the application
while leaving the `edgion.io/health-check` annotation untouched) marks all backends
unhealthy after `unhealthyThreshold × interval` seconds and silently takes the whole
service offline.

This mirrors the failure mode Envoy's **panic threshold** addresses: when the healthy
fraction of a pool drops below a threshold, the health signal more likely reflects a
systemic issue (bad probe, network partition) than genuine backend failure, so routing to
all hosts is strictly safer than concentrating traffic on the few survivors (which then
cascade) or dropping all traffic.

## Design decisions (resolved during brainstorming)

1. **Config surface**: global default in `EdgionGatewayConfig` + per-service override via a
   new `edgion.io/panic-threshold` annotation. Annotation overrides global default.
2. **Default value**: `0` (panic **disabled**, opt-in). This deliberately diverges from raw
   Envoy's 50% default and matches Istio's k8s-native choice (`outlierDetection.minHealthPercent`
   defaults to 0). Rationale: k8s services typically run few pods, where a 50% panic threshold
   misfires — a 2-pod service losing 1 pod hits exactly 50% and would panic, routing traffic
   back to the genuinely-bad pod and overriding a legitimate ejection. Operators with large
   pools opt in explicitly. Strong documentation guidance compensates (see Documentation).
3. **Circuit-breaker interaction**: independent final safety valve. Panic is evaluated over the
   **combined** `is_backend_healthy` predicate (active HC + circuit breaker together). The
   circuit breaker keeps its own `maxEjectionPercent` cap unchanged. No double-counting because
   we count survivors of the combined predicate, not each gate separately.
4. **Where to count**: on-demand O(n) pre-pass per request, gated so it only runs for services
   that actually have health-check or circuit-breaker state, with early-exit once the floor is
   reached. No maintained counter (avoids state-sync drift).

## Architecture

### Effective threshold resolution

Precedence (first match wins):
1. `edgion.io/panic-threshold` annotation on the Service (integer 0–100, validated; invalid →
   warn + ignore, fall through).
2. Global default from `EdgionGatewayConfig` (`loadBalancing.panicThreshold`).
3. Built-in default `0` (disabled).

Semantics:
- `0` → panic disabled: always honor the health filter (current behavior preserved exactly).
- `N` in `1..=100` → panic engages when `healthy_survivors / total < N%`.

`floor = ceil(threshold × total / 100)`. Panic engages when `survivors < floor`.

### Config plumbing

**Per-service annotation** — parsed in
`src/core/gateway/backends/discovery/services/conf_handler_impl.rs`, alongside the existing
health-check and circuit-breaker annotation parsing, in both `full_set` and `partial_update`.
Stored in a new lightweight global store (`DashMap<String, u8>`) with `get` / `set` / `remove`;
the entry is cleared when the service is removed (same lifecycle as CB/HC config).

**Global default** — a new optional `loadBalancing: { panicThreshold: u8 }` sub-struct on
`EdgionGatewayConfigSpec`. On apply, the `EdgionGatewayConfigHandler` pushes the scalar into a
process-global `AtomicU8` via a `configure_lb_panic_default(...)` function, mirroring the
existing `configure_webhook_global_max` pattern. The selection path reads this atomic — no
GatewayClass identity needs to be threaded onto the hot path.

### Selection path

In `pick` and `select_roundrobin_backend_key`, before calling `selector.select`, compute the
panic decision **once**:

1. **Gate**: skip entirely unless `health_store.has_service(key) || cb_store.has_service(key)`.
   Without HC/CB state every backend passes `is_backend_healthy`, so panic can never trigger and
   the O(n) pass is pure waste.
2. **Resolve threshold** (annotation → global → 0). If `0`, skip (no panic).
3. **Count survivors** over `cell.backends_snapshot()` using a side-effect-free predicate,
   early-exiting as soon as `survivors >= floor`.
4. **Decide**: if `survivors < floor`, panic engages → pass a no-op filter (`|_| true`) to
   `selector.select`; otherwise pass the normal `is_backend_healthy` filter.

The single resolved filter closure is handed to `CompiledSelector::select`, so all four policy
arms (RoundRobin, ConsistentHash, LeastConn, EWMA), the ConsistentHash empty-key RR fallback,
and the `select_roundrobin_backend_key` path (TCP / UDP / TLS routes, request_mirror plugin)
share identical panic behavior and cannot drift.

### Predicate split (avoid double-counting CB metric)

`is_backend_healthy` currently records a circuit-breaker rejection metric
(`record_cb_rejection`) as a side effect when a backend is ejected. The O(n) counting pass must
not re-fire this metric. Split into:

- `is_backend_healthy_quiet(...)` — pure predicate, no metric. Used by the counting pass.
- The selection-path filter keeps the metric on rejection (wrapping the quiet predicate), so the
  existing CB rejection metric semantics on the actual selection walk are unchanged.

### Observability (Iron Rule 3: no `tracing::` on the request path)

- **Hot path**: the panic decision flips a per-service `AtomicBool panic_active` and sets a
  gauge metric `lb_panic_mode{service}` (1 engaged / 0 normal). Atomics only — no logging.
- **Off path**: the health-check manager's existing periodic reconcile reads `panic_active` and
  emits a `tracing::warn` only on an engage/disengage **edge**, so operators see
  "panic mode engaged/cleared for service X" without any logging on the request path.

### Status-code semantics

When panic engages, selection draws from the full pool ignoring health. The request proceeds to
connect; a genuinely-dead backend fails to connect and flows into the **existing** retry/error
path (`maxRetries` from `EdgionGatewayConfig`). This is strictly better than today's blanket
`not_found`: probe-misconfigured-but-healthy backends resume serving, and truly-dead backends
get retried instead of a hard 503. No new status code. The `not_found` mapping is retained only
for the genuinely-empty pool (no backends at all), which is reached before the panic logic runs.

## Edge cases

- `total == 0`: handled before panic logic (`hydrate_cell_if_empty` returns false → `not_found.empty`).
- `threshold == 100`: `floor == total` → panic unless **all** backends are healthy. Valid extreme.
- All backends healthy: survivors reach `floor` within `floor` iterations, early-exit, no panic.
- Invalid annotation value (non-integer, > 100): warn + ignore, fall back to global default.

## Testing

**Unit**
- `floor` math: ceil rounding, `0` disables, `100` boundary, value clamping/validation.
- Counting pass: early-exit at floor; `is_backend_healthy_quiet` fires no CB metric.
- Per-policy panic-on + all-unhealthy → a backend is selected for RoundRobin, ConsistentHash
  (keyed and empty-key fallback), LeastConn, EWMA, and `select_roundrobin_backend_key`.
- Panic-off (threshold 0) + all-unhealthy → still `not_found` (current behavior preserved).
- Partial unhealthy above floor → health filter honored, no panic.
- Threshold resolution precedence: annotation > global default > built-in 0.

**Integration**
- Reproduce the misconfigured-health-check scenario: a service whose probe path no longer exists
  marks all backends unhealthy; with `edgion.io/panic-threshold: "50"` the service stays up via
  panic mode, and with no annotation (default 0) it returns the pre-existing `not_found`.
- Add the scenario under `skills/05-testing`.

## Documentation (strong opt-in guidance — required, not optional)

Because the default is `0` (disabled), documentation must actively steer operators:

- Document `edgion.io/panic-threshold` and `loadBalancing.panicThreshold` in the annotations and
  EdgionGatewayConfig references.
- State the k8s "few pods per service" caveat explicitly: small pools should leave panic off or
  use a low/large value deliberately; large pools (≈ ≥ 5 backends) are where 50% is appropriate.
- Use the misconfigured-health-check outage as the canonical "why you might opt in" example.
- Cross-link from the circuit-breaker and health-check docs so operators configuring either gate
  discover the panic safety valve.

## Out of scope

- Changing the circuit breaker's internal `maxEjectionPercent` logic (panic stays independent).
- A maintained healthy-count store (rejected in favor of the gated O(n) pass).
- Per-route (vs per-service) panic configuration.
