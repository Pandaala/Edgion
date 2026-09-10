---
name: connect-failure-after-deadline-uses-timeout-outcome
description: Review HTTP upstream TCP/TLS establishment budgets and typed deadline attribution; request or gRPC deadline exhaustion owns the non-retryable 504 outcome.
status: fixed
knowledge_type: regression-guards
---

# HTTP upstream establishment bounds and deadline outcomes

## Conclusion

`configure_peer_timeouts` bounds TCP plus TLS establishment by the minimum of
the captured backend policy/global connect timeout, remaining route request
budget, and remaining client gRPC budget. Both `connection_timeout` (per phase)
and `total_connection_timeout` (outer L4+TLS bound) receive the result.
`EdgionHttpContext.connect_timeout_source` records its typed owner independently
of the read/write `peer_timeout_source`; `backendRequest` never participates in
the connect fold. Every peer configuration replaces the options and source,
including clearing them when all three bounds are disabled.

Configured zero means disabled. Remaining zero means expired and is retained.
The fold prefers request, then gRPC, then configured connect on equal budgets.

When upstream connection establishment fails after the route request deadline
or client gRPC deadline has expired, the expired end-to-end deadline owns the
terminal outcome. `fail_to_connect` records the canonical deadline diagnostic
and returns a non-retryable `HTTPStatus(504)`, which maps to gRPC
`DEADLINE_EXCEEDED`. The route request deadline has attribution precedence when
both deadlines are expired.

If neither absolute deadline is yet expired, only `ConnectTimedout` or
`TLSHandshakeTimedout` may fall back to the recorded deadline owner, and only
when that owner's remaining budget is at most 1 ms. Pingora also maps OS
`ETIMEDOUT` to `ConnectTimedout`; the error type and configured owner alone do
not prove the end-to-end timer fired. This narrow window handles possible
sub-millisecond early timer firing without suppressing retries after an OS
timeout with ample budget left. A plain connection refusal or TLS validation
error cannot use that fallback. A configured-connect-owned timeout keeps the existing retry
policy unless an end-to-end deadline has actually expired.

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

- Fold only configured connect, request remaining, and gRPC remaining.
- Set the same bound on both phase and total connection timeout options.
- Preserve expired zero budgets and clear stale sources/options on the next peer.
- Use the captured policy generation, never a live backend-policy store read.

- Select `RequestTimeoutExceeded` before `GrpcClientDeadlineExceeded` when both
  deadlines are expired.
- Stamp the current attempt 504 without replacing its original error or timing.
- Wrap the original error as the cause of a fixed-context `HTTPStatus(504)` and
  explicitly force the outer error non-retryable.
- Do not move the deadline branch ahead of attempt/outlier accounting or behind
  relay, AI, or retry-budget decisions.
- Do not add request-path tracing or a deadline-specific metric; the canonical
  request diagnostic and access log own observability.
- Check both deadlines against one `Instant` sample at failure. The early-timer
  fallback checks only the recorded owner's remaining budget, never the other
  deadline, and cannot apply when that budget is missing or exceeds 1 ms.

Focused unit tests cover the four selector combinations and both typed outcomes,
including exact diagnostics, attempt status, forced non-retryability, and the
retained original error type/source.

## Dependency and verification limits

The locked Edgion Pingora `518f727` implementation wraps `do_connect_inner`
with `total_connection_timeout` in `connectors/mod.rs::do_connect`, covering
L4 connect and TLS handshake. It does not wrap connector offload dispatch,
pool acquisition, or the later HTTP/2 client handshake. This is not an absolute
deadline for the whole `get_http_session` call or a streaming response.

The fast timer floors the absolute expiry to milliseconds before rounding to
a 10 ms tick. It can fire less than 1 ms early; it can also be late due to tick
resolution and scheduling. Typed attribution avoids retrying on the early edge.

The stalled-handshake integration case requires an actual TLS ClientHello,
checks elapsed time, HTTP/gRPC terminal status, deadline diagnostics, a single
upstream attempt despite enabled retries, and transport closure. It uses one
bounded local backend with the shared Controller/Gateway topology. It does
not establish TCP-blackhole, offload, or HTTP/2-handshake timing coverage.

## Re-evaluation triggers

- Pingora changes `HTTPStatus(504)` or gRPC deadline error mapping.
- Pingora changes the outer connection timer, its resolution, or error types.
- The whole connector operation requires an absolute deadline; assess offload,
  pool and HTTP/2 setup separately rather than extending the claim silently.
- Deadline attribution precedence changes across the shared retry paths.

## References

- `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs`
- `edgion-gateway/src/runtime/server/error_response.rs`
- [gRPC timeout handling](../../h2-grpc/regression-guards/timeout-honored.md)

## Resolution provenance

`bound-http-upstream-connect-by-end-to-end-deadlines` was a real P2 defect:
the earlier correction fixed terminal classification but still waited for the
independent connect timeout. The follow-up adds the L4+TLS bound and typed
attribution described above. The historical terminal-classification correction
remains necessary for non-timeout errors arriving after deadline expiry.

## Verification (2026-09-10)

- `cargo test -p edgion-gateway --lib routes::http::proxy_http::pg_ --no-default-features --features boringssl,allocator-system`:
  247 passed after the OS-timeout attribution correction. This includes connect
  folds, actual peer configuration/reset, and the OS timeout/early-window tests.
- The same freshly built libtest executable ran `ctx::ai_context_claim_tests`:
  5 passed, including the 1120-byte context and allocation-free construction guards.
- Gateway and test-client binaries built successfully; the Gateway was rebuilt
  after the final attribution correction.
- Shared local `start_all_with_conf.sh --suites EdgionBackendTrafficPolicy/ConnectTimeout`
  followed by `test_client -g -r EdgionBackendTrafficPolicy -i ConnectTimeout`:
  3 tests passed, no infrastructure errors. The new test exercised route-only,
  gRPC-only and equal request/gRPC 300 ms deadlines against a confirmed stalled
  ClientHello with a 5 s configured connect timeout and 1 ms backendRequest.
  It checked the 150 ms–2 s elapsed envelope, terminal HTTP/gRPC status,
  canonical diagnostic, single attempt despite retries, and backend closure.
  The shared topology was stopped after the test.
- The shared launcher/readiness/resource-diff path was invoked directly because
  the top-level runner requires macOS loopback aliases used by unrelated suites.
  This fixture uses only `127.0.0.1`; no network settings or runner files changed.
  English-only, resource-type purity and shell-portability preflights passed separately.
- Format apply/check, workspace all-target check and clippy, agent-doc, SSA-force,
  metrics inventory, Pingora source policy and whitespace checks passed. Clippy
  retains existing workspace warnings; no warning cleanup was included.
- Two independent code reviewers completed review and re-review. The second
  found the OS `ETIMEDOUT` error-type overlap; after the bounded remaining-time
  check and regression test were added, both reviewers returned LGTM.

No full integration sweep or `cargo test --all` was run. OS timeout attribution
with a long remaining budget is unit-tested, not a real long-running SYN test.
