---
name: connect-failure-after-deadline-uses-timeout-outcome
description: Use when reviewing fail_to_connect deadline handling; an expired route request or gRPC deadline owns the terminal 504 outcome even when connection establishment reports a different error.
status: fixed
knowledge_type: regression-guards
---

# Connect failure after an expired deadline uses the timeout outcome

## Conclusion

When upstream connection establishment fails after the route request deadline
or client gRPC deadline has expired, the expired end-to-end deadline owns the
terminal outcome. `fail_to_connect` records the canonical deadline diagnostic
and returns a non-retryable `HTTPStatus(504)`, which maps to gRPC
`DEADLINE_EXCEEDED`. The route request deadline has attribution precedence when
both deadlines are expired.

The original connection error remains the cause of the 504 for binary-owned
error logging. Its type and upstream source are not exposed in the client error
body. The current attempt retains its original error and elapsed-time fields,
while its status is normalized to 504.

## Why this is required

The old deadline branch suppressed retry and stamped the attempt as 504 but
returned the original connection error. A non-timeout upstream `ConnectError`
was then classified by `fail_to_proxy` as 502, so HTTP clients received the
wrong status and gRPC clients missed the canonical status 4 mapping. The branch
also omitted `RequestTimeoutExceeded` or `GrpcClientDeadlineExceeded`, leaving
the access log without the actual terminal cause.

The conversion occurs after attempt and local-origin outlier stamping and
before relay eligibility, AI successor selection, retry counts, or retry-budget
consumption. Ordinary connection failures therefore keep their existing
behavior, and an expired request cannot acquire retry-owned side effects.

## Regression guards

- Select `RequestTimeoutExceeded` before `GrpcClientDeadlineExceeded` when both
  deadlines are expired.
- Stamp the current attempt 504 without replacing its original error or timing.
- Wrap the original error as the cause of a fixed-context `HTTPStatus(504)` and
  explicitly force the outer error non-retryable.
- Do not move the deadline branch ahead of attempt/outlier accounting or behind
  relay, AI, or retry-budget decisions.
- Do not add request-path tracing or a deadline-specific metric; the canonical
  request diagnostic and access log own observability.

Focused unit tests cover the four selector combinations and both typed outcomes,
including exact diagnostics, attempt status, forced non-retryability, and the
retained original error type/source.

## Out of scope

Connection establishment is not yet clamped by the remaining end-to-end
deadline. That wider peer-timeout change and its integration coverage are
tracked separately; this guard only fixes terminal classification once a
connection failure reaches `fail_to_connect`.

## Re-evaluation triggers

- Pingora changes `HTTPStatus(504)` or gRPC deadline error mapping.
- Connection establishment gains a typed deadline owner that makes this
  post-failure selection redundant.
- Deadline attribution precedence changes across the shared retry paths.

## References

- `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs`
- `edgion-gateway/src/runtime/server/error_response.rs`
- [gRPC timeout handling](../../h2-grpc/regression-guards/timeout-honored.md)
