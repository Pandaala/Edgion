---
module: routing
title: Connection-error retry fires unconditionally for non-idempotent methods (POST, PATCH, DELETE)
severity: medium
envoy_reference: Envoy route.RetryPolicy — retry_on conditions; "retriable-status-codes", "connect-failure", "reset", "retriable-safe-request" only retry safe methods unless explicitly opted in
---

# Connection-error retry fires unconditionally for non-idempotent methods

## Envoy's design (the proven baseline)

Envoy separates retry conditions by request safety. For connection-level failures
(`connect-failure`, `reset`, `refused-stream`), Envoy only retries safe/idempotent
methods (GET, HEAD, OPTIONS, TRACE) by default. For non-safe methods (POST, PUT,
PATCH, DELETE), the operator must explicitly opt in via `retriable-request-headers`
or by not using the `retriable-safe-request` guard. The rationale is that retrying a
non-idempotent request after a mid-flight connection reset risks double-execution —
money transferred twice, order placed twice, row inserted twice.

## Edgion's current design

`pg_fail_to_connect.rs` (`fail_to_connect`, line 65):

```rust
if ctx.try_cnt <= max_retries {
    e.set_retry(true);
}
```

`error_while_proxy.rs` (`error_while_proxy`, line 50):

```rust
let can_retry = client_reused && buffer_ok && ctx.try_cnt <= max_retries && !deadline_exceeded;
```

Neither guard consults the HTTP method. The retry decision is purely a count check
(bounded by `rule.retry.attempts` or the global `max_retries` default of 3). There
is no code path that exempts `POST`, `PUT`, `PATCH`, or `DELETE` from connection-error
retries. A `ParsedRouteRetry` carries only `attempts`, `backoff`, and `codes`; there is
no `retriable_methods` or `safe_only` field.

## The problem / gap

When a POST request reaches an upstream that accepts the connection then resets it
before sending a response (or when a keepalive connection goes stale mid-request),
Edgion retries the full request automatically. The backend may have already started
executing the mutation. The second attempt duplicates it.

The `retry.codes` feature (status-code-based retry) has the same gap: a 503 on a POST
body is retried without an idempotency guard or explicit operator opt-in.

There is also no `retriable-request-headers` equivalent: operators cannot express
"retry only if the upstream echoes back an `x-request-idempotency-key` header" or
"retry only if the backend adds a `retry-safe` response header".

## Impact

- Silent double-writes for any non-idempotent route that has retries enabled globally
  or via `rule.retry`.
- Severity is amplified because the global `max_retries` default (3) applies to all
  routes; operators who have not explicitly set `retry.attempts: 0` on POST routes are
  exposed.
- Particularly dangerous for payment, order, or mutation APIs proxied through Edgion.

## Verification note

Verified against actual source (`pg_fail_to_connect.rs` lines 65–67, `pg_error_while_proxy.rs`
line 50). Neither function checks `session.req_header().method` before setting the retry flag.

The risk profile differs between the two code paths:
- **`fail_to_connect`** (connection never established): the request body was never sent to the
  backend, so retrying a POST is actually safe — no double-execution can occur. The finding
  overstates the risk for this path.
- **`error_while_proxy`** with `client_reused = true` (stale keepalive connection reset after
  body was sent): this IS the genuine double-execution risk. Pingora's retry buffer captures
  up to `BODY_BUF_LIMIT` (64 KB) of the request body. Any POST with a body ≤ 64 KB on a
  reused keepalive connection can be silently re-sent to the backend after a reset. The
  `buffer_ok` guard only prevents retry when the body exceeds 64 KB — it does not guard
  on HTTP method.

The `ParsedRouteRetry` struct (`http_route.rs` lines 849–856) confirms there is no
`retriable_methods` or `safe_only` field. The core claim stands for the `error_while_proxy`
path; the `fail_to_connect` concern is lower severity than stated.

## Suggested direction

1. Add an HTTP-method safety guard in `error_while_proxy` (the genuine double-execution
   path): when `client_reused = true` and the method is non-idempotent (POST, PATCH,
   DELETE), do not set retry unless an explicit opt-in field is present on the route's
   retry config (e.g., `retriable_methods: ["POST"]`). The `fail_to_connect` path does
   not need this guard because the body was never sent to the upstream.
2. Introduce a `retriable_methods` field on `HTTPRouteRetry` (or a boolean
   `allow_non_safe_retry`). When absent, default to safe-methods-only for the
   `error_while_proxy` stale-keepalive path.
3. For the global `max_retries` path, consider documenting that it applies to all methods
   and operators should set `retry.attempts: 0` on non-idempotent routes if they have not
   explicitly opted in.
