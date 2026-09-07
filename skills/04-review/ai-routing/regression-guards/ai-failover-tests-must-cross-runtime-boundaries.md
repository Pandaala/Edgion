---
knowledge_type: regression-guards
---

# AI failover tests must cross runtime boundaries

Pure scheduler and limiter tests do not prove that dispatch adapters call them
at the correct lifecycle boundary. Keep discriminating tests that cross each
production seam:

- a newly started dispatch contributes exactly one retry-budget denominator
  sample, and budget denial stops the scheduler with `RetryDenied`;
- exhausted credential and model eligibility reaches `Unavailable` without
  consuming a dispatch;
- retry-cap initialization preserves both the resolved cap and scheduler
  progress when called again;
- credential success resets failure counters, completes half-open probes only
  after successful EOS, and releases max-ejection capacity;
- admission rejects missing outlier state, an already-active attempt,
  accounting overflow, and fail-close Redis dependency failure;
- every attempt outcome chooses an explicit `Release`, `Reconcile`, or `Retain`
  decision, and that decision maps independently to the Redis operation.

Tests do not need a live Redis client to observe settlement selection. A
`cfg(test)` decision trace can record the selected operation before the
no-reservation fast return, while a pure mapping test guards the Redis command
translation. This avoids fake network clients and keeps production behavior
unchanged.

## Resolution references

- `test-04` — 2026-08-09.
