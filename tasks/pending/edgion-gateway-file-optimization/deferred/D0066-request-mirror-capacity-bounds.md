# D0066 — Bound RequestMirror Concurrency and Buffering

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0102, RequestMirror analysis
- Category: configuration-correctness
- Canonical task: [`tasks/todo/bound-request-mirror-concurrency-and-buffering.md`](../../../todo/bound-request-mirror-concurrency-and-buffering.md)

## Evidence
RequestMirror accepts `maxConcurrent` and `maxBufferedChunks` as unconstrained `usize` values. Values above Tokio's supported semaphore range can panic during construction, and an oversized channel capacity can panic or defeat the intended memory bound when a request is armed.

## Why deferred
One canonical limit must cover HTTPRoute annotation parsing, EdgionPlugins validation, defensive Gateway construction, runtime channel creation, and both documentation sets. Tightening only the request path would retain inconsistent admission and panic behavior.

## Integration-test impact
No integration issue is created now. Deterministic resources, Controller/Gateway parity, construction, and channel-boundary tests should cover the future validator; add a separate integration issue only if the approved design changes observable admission behavior that cannot be proved locally.
