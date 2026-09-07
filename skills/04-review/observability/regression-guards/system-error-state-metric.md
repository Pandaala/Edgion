---
name: system-error-state-metric
description: Use when reviewing findings about unsafe modes (skip_tls, AllowInsecureFallback, etc.); documents when to close a report by reusing the edgion_system_error_state gauge rather than adding a hard constraint.
knowledge_type: regression-guards
---

# `edgion_system_error_state` Gauge: Registry of System-Level Degraded States

## Conclusion

Edgion uses the `edgion_system_error_state{component, reason}` gauge to register **"the process is running but in a known degraded/unsafe state"**. The typical use case is the emergency TLS bypass `skip_tls=true` — the behavior itself is preserved (operators have a legitimate emergency-escape-hatch need); the metric lets Prometheus/Alertmanager catch "degraded mode is active + not turned off in time".

When a review reports "Edgion permits an unsafe mode somewhere (`skip_tls`, `AllowInsecureFallback`, etc.); should refuse startup at the code level / require two-factor confirmation / enforce a time limit", apply the rules below to decide whether to close it by reusing this mechanism rather than introducing a new hard constraint.

## Design Constraints

1. **Metric name**: `edgion_system_error_state` (gauge; `1` while the condition is active)
2. **Labels**: `component` (reporting subject) and `reason` (condition type), both defined in
   `edgion-common/src/observe/system_metrics.rs`. **That module is the only source of truth for
   which values exist — read it rather than trusting a list here.** This page previously
   enumerated them and the list rotted (it named a `fed_sync_server` component that does not
   exist and listed 2 reasons when there were 9); an enumeration in a review doc is a
   maintenance trap, so there is deliberately none now.
3. **Label values must be drawn from bounded constants**; passing free-form strings is not allowed (Prometheus cardinality risk)
4. **Most reasons latch, but not all**: a startup *configuration* degradation (`skip_tls`) is set
   once and never zeroed — process-restart semantics cover it. Other reasons track a *transient
   runtime* condition and are explicitly zeroed by their success path via `clear_system_error`
   (e.g. `confsync_disconnected`, `cert_reload_invalid`, `cert_rotation_failed`). Each reason's
   doc comment states which it is. Alert on a **sustained** `== 1` for the transient ones — a
   single blip is normal.
5. **A latching reason needs a single owner, and the owner must be the process that is actually
   broken.** The gauge is keyed by `(component, reason)` and nothing else, so a condition observed
   once *per peer* (one controller, many gateways) cannot be a reason here: every healthy peer's
   success clears the broken peer's mark and the alert can never fire. A `peer` label is not the
   escape — on a pre-auth path any unauthenticated caller could then mint unbounded series. Let
   the broken peer flag itself on its own registry, and leave per-peer detail in a rate-limited
   log. Full rule + a worked example (the 2026-07-16 `tls_handshake_failed` removal):
   [`../../03-coding/observability/01-metrics.md`](../../../03-coding/observability/01-metrics.md).
6. **Pair with `tracing::error!`**: log gives a one-shot startup warning; the metric provides continuous observability; both coexist

## When to Use This Mechanism

**Applicable** (use the metric to register + preserve the behavior):
- Degraded modes with **clear emergency-escape-hatch value** (operators can use them in extreme scenarios)
- Refusing at the code level would block legitimate use cases
- Monitoring is sufficient to catch "forgot to turn off" promptly
- The condition is owned by the **emitting process** — either a startup configuration decision, or
  a transient runtime state that the same process both raises and clears (see constraint 4)

**Not applicable** (should refuse at the code level or refactor):
- Degradation has no legitimate use case (purely legacy; deletion is lossless)
- The condition is observed **per peer** rather than per process (see constraint 5) — the mark
  would be cleared by healthy peers and could never sustain an alert
- Degradation **flaps** on a hot path (a state gauge answers "is it broken now", not "how often" —
  if you need a rate, that is a counter, and a counter is a new metric family requiring
  arbitration per `01-metrics.md`). Note this is about flapping, **not** about being transient:
  a transient condition with a clear owner and a `for:`-guarded alert is squarely applicable.
- Degradation involves user-controllable input (the attack surface is larger; an escape hatch should not exist)

## Decision Rule: When Review Says "XX Unsafe Mode Should Be Forbidden at Code Level"

Decide in the following order:

1. **Is the metric already reported?**
   - Check whether `edgion-common/src/observe/system_metrics.rs::components` covers the report point
   - Check whether `system_metrics.rs::reasons` covers this degradation type
2. **Does it match the "applicable" scenarios** (see above)?
3. All three are yes → point to this file + `tasks/security-audit/04-admin-api.md` M2 decision and close
4. If the metric does not yet cover the review point → assess whether to **add a new reason constant** (rather than a new independent metric); after adding, close per the flow above

## Exceptional Scenarios (need reassessment)

- The degraded mode starts to show frequent flips (toggled at runtime repeatedly): gauge semantics no longer apply; consider switching to counter or event log
- `component` / `reason` cardinality exceeds 20: Prometheus overhead grows; reassess whether labels need to be merged or layered
- A degraded mode is shown to "have no legitimate use case at all": upgrade from metric registration to code-level refusal

## Background Notes

- Decision date: 2026-04-22
- Decision source: `tasks/security-audit/04-admin-api.md` M2 (gRPC `skip_tls=true` plaintext connection)
- Implementation file: `edgion-common/src/observe/system_metrics.rs`
- First-batch covered sites: 5 `skip_tls=true` branches (Gateway / Controller / Center)
- Typical Alertmanager rule (configured on the operator side, not built into Edgion):
  ```yaml
  - alert: EdgionSystemErrorState
    expr: edgion_system_error_state > 0
    for: 5m
    annotations:
      summary: "Edgion {{ $labels.component }} running in degraded mode ({{ $labels.reason }})"
  ```
