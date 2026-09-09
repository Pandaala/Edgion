# B0214 — UDP Route Unit

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/route_unit.rs`.
- Baseline: `86e2002c`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0214 analyst.
- Investigate immutable schema/selector ownership, plugin and annotation policy,
  and actual UDP session consumers; revalidate after B0211 handler changes.
- No implementation authorized; existing integrations and user-owned paths remain protected.

## Analysis Outcome

- Decision: target examined-no-change with simple owning-knowledge correction. Target blob `8e7289a8ccc840eaa43c9013597a096a03b9fb32` at `86e2002c`; analyst did not review the concurrently active B0211 diff or run tests.
- Unit retains schema Arc and completes shared selector/key compilation before publication; first-rule, empty-backend, mixed-weight and denied/unsupported post-select policies match actual UDP consumers. Outer selector Arc removal offers insufficient cold-path value.
- Exact proposed technical scope is one original knowledge page: `skills/04-review/routing/regression-guards/udproute-stream-plugins-annotation-wiring.md`. Correct stale rule/handler ownership to UdpRouteUnit, nonexistent legacy runtime fields, old F-016 link and re-report checkpoints.
- Critically, 60 seconds is an inactivity threshold, not a maximum lifetime or guaranteed hot-reload-to-deny delay. Active sessions can retain their established decision; describe new-session dynamic store lookup without adding immediate invalidation requirements.
- Preserve useful historical evidence and current code/integration behavior. Existing broad manager-doc task does not cover this incorrect timeout claim; no duplicate issue required.
- At promotion revalidate B0211/B0212 changes and independently review the single-page correction. Unit five-test filter and shared selector are available for fresh smoke verification.
