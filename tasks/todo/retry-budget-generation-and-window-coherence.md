# Make retry-budget generations and window rotation coherent

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Redesign `RetryBudget` so one retry decision uses one monotonic instant and one coherent counter epoch. The current rotation publishes a new `window_start` before it swaps the attempts and retries counters, so a racing caller can observe a new epoch with old or half-rotated counters, and an increment made after the epoch CAS can be moved into `previous`. `try_retry` also reads wall time independently for every estimate and increment despite documenting one clock read per public operation. Core Service requests can additionally count an attempt in one live policy generation and consult another generation at the retry gate after hot reload.

## Requirements

- Use monotonic elapsed time for in-process retry windows; wall-clock changes must not extend, resurrect, or prematurely expire a budget window.
- Thread one logical `now` through every attempts, retries, and minimum-rate operation in one `on_attempt` or `try_retry` call.
- Publish the epoch and both budget counter pairs as one coherent state transition, or use a protocol that prevents readers and incrementers from observing a partially rotated state.
- Keep increments after a rotation boundary in the new current window.
- Clear `previous` when two or more complete intervals elapsed; traffic older than one prior window must not affect interpolation.
- Preserve the documented soft-cap policy: check-and-increment may over-admit by at most the explicitly accepted concurrent-decider bound, but rotation races must not add another source of skew.
- Capture one `Arc<RetryBudget>` for the relevant Service attempt/request generation so the attempt denominator and its later retry decision cannot land in different hot-reload generations. Preserve the existing request-stable AI policy capture.
- Keep the request path allocation-free, non-blocking, and free of tracing.

## Scope

### In scope

- `backends/policy/backend_traffic/retry_budget.rs` counter and clock model.
- Core Service and AI retry-budget capture and call sites.
- Deterministic clock and concurrency regression tests.

### Out of scope

- HTTPRoute retry eligibility, retry counts, or backoff semantics.
- Changing the configured percentage or minimum-rate vocabulary.
- Adding a fifth failure-policy shape.

## Acceptance criteria

- A barrier-driven test that pauses a rotator after it wins the epoch transition proves that a racing new-window increment remains in `current`.
- No observer can see attempts and retries from different rotation phases.
- Instrumented tests prove that each public operation samples time exactly once.
- Advancing by at least two intervals produces an empty previous window.
- A Service policy hot reload between dispatch and failure cannot split one attempt's denominator and retry decision across budget Arcs.
- Existing percentage, minimum-rate, Service, and AI retry tests retain their intended results.
- Workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/05-resources/20-edgion-backend-traffic-policy.md`
- `skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`
- `skills/03-coding/03-time-conventions.md`
- `skills/04-review/concurrency/sliding-window-check-then-increment.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer implementation from the file audit. | A local ordering or timestamp patch cannot make the epoch, four counters, floor window, and request generation coherent without selecting and validating a complete non-blocking state protocol. |
| 2026-08-31 | Preserve the accepted soft check-then-increment bound. | `sliding-window-check-then-increment.md` closes the separate single-counter admission race with a CAS loop. This task does not reopen that decision; it addresses multi-counter epoch publication, multi-window expiry, and generation capture. |
