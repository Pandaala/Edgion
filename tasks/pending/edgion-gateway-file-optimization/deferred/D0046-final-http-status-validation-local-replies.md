# D0046 — Use Canonical Final HTTP Status Validation for Local Replies

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0075, FaultInjection audit
- Category: correctness-unification
- Depends on: B0075
- Canonical task: [`tasks/todo/use-canonical-final-http-status-validation-for-local-replies.md`](../../../todo/use-canonical-final-http-status-validation-for-local-replies.md)

## Evidence
FaultInjection, Mock, RequestAccessPolicy, RateLimit, RateLimitRedis, IpRestriction, and RequestRestriction resource schemas or validators accept status codes from 100 through 599 for request-stage local replies. The canonical runtime final-response predicate accepts only 200 through 599. A 1xx status cannot claim the final response, so both runtime-owned `ErrResponse` handling and plugins that write a local response before termination may proceed toward body write/termination without a valid final header.

## Why deferred
The correction spans seven shared resource configurations, generated schema ranges, their colocated tests, and schema-facing status documentation. It should reuse the canonical final-status predicate and preserve Controller/Gateway validation parity instead of adding plugin-local checks or changing the runtime response writer.

## Integration-test impact
No new integration issue is required. Shared resource validation and plugin-compilation tests can prove 1xx rejection before publication while existing integration tests remain unchanged.
