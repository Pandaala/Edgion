# D0019 — Make Webhook Sliding Windows Time-Coherent

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0053, Webhook runtime rate limiting
- Category: correctness

## Evidence
`SlidingWindowCounter` rotates only one previous bucket after an arbitrarily long idle period, so traffic older than two windows can still influence the next decision. Its epoch wall-clock source can also move backward independently of elapsed runtime.

## Why deferred
Correcting the behavior requires a deliberate time-source and interpolation contract shared with any future outbound limiter primitive; it is separate from the backend retry-budget generation task.

## Candidate next step
Define monotonic elapsed-window semantics, clear expired history after multiple idle windows, and add deterministic clock-driven tests before unifying only consumers with equivalent policies.
