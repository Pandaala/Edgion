# D0028 — Define the Redis Rate-Limiter Numeric Domain

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0061, shared Redis rate-limiter audit
- Category: correctness-security
- Canonical task: [`tasks/todo/define-redis-rate-limiter-numeric-domain/`](../../../todo/define-redis-rate-limiter-numeric-domain/define-redis-rate-limiter-numeric-domain.md)

## Evidence
HTTP and stream rate-limit configurations accept `u64` rates, a `u32` HTTP cost, and duration strings whose derived millisecond and arithmetic values lack one exact shared upper-bound contract. Values can exceed Lua's exact double-integer domain, Redis signed integer operations, reply decoding, or the stream per-listener local gate's capacity/refill arithmetic and `u128`-to-`u64` conversion bounds. Runtime rounding, overflow, or command/reply failure can then enter the configured dependency failure policy, including fail-open.

## Why deferred
The fix spans two resource configurations, DAG-lower canonical constants, schema parity, the shared Gateway core defense, HTTP and stream caller preparation, the local per-listener gate, derived arithmetic, documentation, and boundary tests. A script-local clamp or duplicate Gateway constant would silently drift from admitted configuration.

## Integration-test impact
Existing integration tests remain frozen. Numeric boundary behavior is deterministic in resource and Gateway unit tests; no separate numeric-domain integration issue is needed.
