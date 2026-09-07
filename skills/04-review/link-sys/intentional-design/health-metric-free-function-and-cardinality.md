---
name: health-metric-free-function-and-cardinality
description: Use when reviewing LinkSys health-metric findings; covers why `edgion_link_sys_health` uses a free-function `gauge!`/`counter!` (not a OnceLock struct handle) and why the per-`link` label is cardinality-safe.
status: accepted-tradeoff
finding_id: link-sys-kafka-health-02
closed: 2026-06-16
knowledge_type: intentional-design
---

# LinkSys health metric: free-function emit + per-link label — NOT-AN-ISSUE

## Conclusion

`record_link_sys_health(name, healthy)` (gauge `edgion_link_sys_health`) and
`record_link_sys_health_flip(name, to_healthy)` (counter `edgion_link_sys_health_flips_total`) in
`edgion-gateway/src/observe/metrics/registry.rs` are emitted via free-function `gauge!`/`counter!`
with a per-provider `link` label (and a fixed `direction` label on the counter). This is correct.
Suggestions to register them as `OnceLock` handles on `GatewayMetrics`, or to drop the `link` label
over cardinality fears, are **not accepted**.

## Core Rationale

**1. Free-function `gauge!`/`counter!` is the established precedent for flip/event metrics.**

`record_cb_ejection`/`record_cb_recovery` and the existing
`record_link_sys_es_bulk_channel_full` all use the free-function form. These fire on state changes
or rare events, not per-request, so the macro's internal handle lookup is not a hot-path cost. A
gauge only ever `set` on a flip also stays absent from `/metrics` until the first emit — the same
"no cold zero-noise" benefit the `OnceLock` policy exists to provide for the struct handles. Adding
struct fields would be inconsistent and buy nothing.

**2. The `link` label is bounded by provider count.**

One series per configured LinkSys provider — the same bounded shape as the existing `service`
(circuit breaker / LB degrade) and `kind` (config parse) labels in this file. `direction` is a fixed
2-value set (`recovered`/`degraded`). No open-ended/user-controlled label values. Not a cardinality
risk.

**3. Stale series on provider deletion is a known, accepted, pre-existing behavior.**

A deleted provider can leave `edgion_link_sys_health{link="..."}` at its last value
until configured idle eviction removes the series. With idle eviction disabled,
it can remain for the process lifetime. The shared production recorder supports
idle eviction for all metric kinds; the earlier blanket non-expiration claim was
incorrect. This acceptance is specific to LinkSys;
it does not justify stale current-state metrics in other mechanisms.
Bounded by providers-ever-seen; not a regression introduced by this change.

The former LB gauge was incorrectly listed here as a cold-path precedent: its
writer ran during request selection. It has been replaced by a cached event
Counter; see [Service degrade observability](../../lb/regression-guards/degrade-selection-counter-ownership.md).

## Fix Suggestions Not Accepted

- "Register these as `OnceLock<Gauge>`/`OnceLock<Counter>` on `GatewayMetrics`" — inconsistent with
  the flip/event-metric precedent; no hot-path benefit since they fire only on transitions.
- "Drop the `link` label to avoid cardinality blowup" — cardinality is bounded by provider count;
  dropping it makes per-provider health undiagnosable.
- "Expire the stale series on provider deletion" — out of scope, pre-existing registry behavior
  shared by all per-key gauges; not specific to this metric.

## Re-evaluation Triggers

Re-open only if:

- A future caller emits one of these metrics on a per-request (hot) path — then reassess handle
  registration and label cost.
- A label is added whose value is operator/user-controlled or otherwise unbounded.

## Reference Cases

- 2026-06-16 Kafka health-recovery fix (spec `docs/superpowers/specs/2026-06-15-kafka-health-recovery-design.md`).
- Precedent: `edgion-gateway/src/observe/metrics/registry.rs` (`record_cb_ejection`, `record_cb_recovery`, `record_link_sys_es_bulk_channel_full`).
