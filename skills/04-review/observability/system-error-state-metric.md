---
name: system-error-state-metric
description: Use when reviewing findings about unsafe modes (skip_tls, AllowInsecureFallback, etc.); documents when to close a report by reusing the edgion_system_error_state gauge rather than adding a hard constraint.
---

# `edgion_system_error_state` Gauge: Registry of System-Level Degraded States

## Conclusion

Edgion uses the `edgion_system_error_state{component, reason}` gauge to register **"the process is running but in a known degraded/unsafe state"**. The typical use case is the emergency TLS bypass `skip_tls=true` — the behavior itself is preserved (operators have a legitimate emergency-escape-hatch need); the metric lets Prometheus/Alertmanager catch "degraded mode is active + not turned off in time".

When a review reports "Edgion permits an unsafe mode somewhere (`skip_tls`, `AllowInsecureFallback`, etc.); should refuse startup at the code level / require two-factor confirmation / enforce a time limit", apply the rules below to decide whether to close it by reusing this mechanism rather than introducing a new hard constraint.

## Design Constraints

1. **Metric name**: `edgion_system_error_state` (gauge; value is always `1` when the condition is active; only resets on process restart)
2. **Labels**:
   - `component`: reporting subject (defined in `edgion-common/src/observe/system_metrics.rs::components`)
     - Currently: `conf_sync_client`, `conf_sync_server`, `fed_sync_client`, `fed_sync_server`
   - `reason`: degradation condition type (defined in `system_metrics.rs::reasons`)
     - Currently: `skip_tls`, `plaintext_default`, `peer_identity_self_check`
3. **Label values must be drawn from bounded constants**; passing free-form strings is not allowed (Prometheus cardinality risk)
4. **Set once and done**: these are **configuration degradations determined at startup**, not transient faults; no zeroing logic is needed (process restart semantics already cover it)
5. **Pair with `tracing::error!`**: log gives a one-shot startup warning; the metric provides continuous observability; both coexist

## When to Use This Mechanism

**Applicable** (use the metric to register + preserve the behavior):
- Degraded modes with **clear emergency-escape-hatch value** (operators can use them in extreme scenarios)
- Refusing at the code level would block legitimate use cases
- The degradation is a startup configuration decision, not transient
- Monitoring is sufficient to catch "forgot to turn off" promptly

**Not applicable** (should refuse at the code level or refactor):
- Degradation has no legitimate use case (purely legacy; deletion is lossless)
- Degradation toggles frequently at runtime (should use counter / event, not state gauge)
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
