---
name: halfopen-probe-toctou-cas
description: Historical passive-only outbound recovery timestamp CAS, superseded by per-destination managed outbound health.
knowledge_type: superseded
---

# Former passive-only recovery timestamp CAS

## Current conclusion

Inactive. `should_halfopen_probe`, the `HealthState` atomics (`healthy`,
`passive_failures`, `last_halfopen`, `backoff_sec`) and the optional
`passive.backoff` block were removed by gw-plug3-01 (2026-11-07). Managed
outbound recovery now uses the canonical `PassiveEjectionState` half-open lease
per destination. Do not apply the former CAS or zero-interval conclusions to
current code. See
[managed outbound per-destination health](../../link-sys/regression-guards/managed-outbound-per-destination-health.md).

## Historical evidence

F-B02 (2026-05-25) replaced a load/check/plain-store admission with one
timestamp compare_exchange so competing callers with the same baseline could
not all win a due window. The page later documented that a zero interval
allowed probing on every call and that the CAS was not a coherent snapshot of
all health atomics. Those limits, and the absence of recovery when `backoff` was
omitted, motivated the replacement.

## Residual constraint

Recovery admission must remain a single-winner lease per destination; a
generation-checked lease (`HalfOpenProbe`) now provides it.
