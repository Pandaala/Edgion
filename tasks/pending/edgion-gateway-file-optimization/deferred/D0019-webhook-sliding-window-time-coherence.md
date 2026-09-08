# D0019 — Make Webhook Sliding Windows Time-Coherent

## Meta
- Status: resolved
- Severity: medium
- Discovered in: B0053, Webhook runtime rate limiting
- Category: correctness

## Original evidence
`SlidingWindowCounter` previously rotated only one previous bucket after an arbitrarily long idle period, so traffic older than two windows could still influence the next decision. Its epoch wall-clock source could also move backward independently of elapsed runtime.

## Owning fix
The overlapping time-source and idle-expiry claim was fixed by
`webhook-sliding-window-epoch-coherence`. The user approved monotonic fixed-grid
windows and one atomic epoch/count/admission state for the existing shared
outbound limiter. No new cross-consumer limiter abstraction is needed.

The reusable evidence and verification belong to the
[outbound window coherence rule](../../../../skills/04-review/concurrency/regression-guards/sliding-window-check-then-increment.md).
Independent review and required project checks passed on 2026-09-07.
