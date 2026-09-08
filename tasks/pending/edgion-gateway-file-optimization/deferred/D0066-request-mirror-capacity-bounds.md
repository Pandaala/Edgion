# D0066 — Bound RequestMirror Concurrency and Buffering

## Meta
- Status: fixed (2026-09-08)
- Severity: high
- Discovered in: B0102, RequestMirror analysis
- Category: configuration-correctness
- Resolution: [RequestMirror resource budgets](../../../../skills/04-review/request-mirror/regression-guards/mirror-resource-budgets.md)

## Original evidence
RequestMirror accepted `maxConcurrent` and `maxBufferedChunks` as unconstrained `usize` values. Values above Tokio's supported semaphore range could panic during construction, and an oversized channel capacity could panic or defeat the intended memory bound when a request was armed.

## Original deferral
The fix required a canonical limit across HTTPRoute annotations, EdgionPlugins validation, defensive Gateway construction, and runtime channel creation. The approved follow-up implemented shared validation and configurable process and per-mirror budgets; the linked regression guard owns current behavior and verification.

## Integration-test impact
Existing integration tests remain frozen. Deterministic configuration, Controller/Gateway parity, construction, ownership, and channel-boundary tests passed. No separate integration gap was identified within this fix.
