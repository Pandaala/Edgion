---
name: timeout-honored
description: gRPC client grpc-timeout parsing, bounded upstream TCP/TLS establishment and I/O, propagation, and their precise enforcement limits.
knowledge_type: regression-guards
---

# h2-grpc-03 — `grpc-timeout` header is honored end-to-end

**Status:** ✅ fixed · 2026-05-18
**Spec:** gRPC-over-HTTP/2 § Request → Timeout (SHOULD for server enforcement, MAY for intermediary propagation)

## Conclusion

Edgion parses the client's `grpc-timeout` header at request entry and stores
`start_time.checked_add(parsed_duration)` in `RequestGrpcInfo.client_deadline`
through `RequestInfo::set_grpc_client_deadline`. It bounds upstream read/write
timeouts by `min(backend_timeout, route_request_remaining, grpc_remaining)`
and re-emits the remaining budget to the upstream. TCP plus TLS establishment
has a separate fold using configured connect timeout instead of backend request
timeout; its typed source is independent of the read/write source.

When the parsed deadline has already elapsed before peer selection (in `pg_upstream_peer::upstream_peer`), the gateway aborts and the client sees DEADLINE_EXCEEDED (`grpc-status: 4`) via the existing 504 → grpc-status 4 mapping.

Parsing and propagation do not prove an absolute kill switch across every
phase. Revalidate a finding against the precise boundaries below. Client
deadlines tighten route budgets; there is no opt-out flag in this mechanism.

## Spec clause

> "Timeout — grpc-timeout — TimeoutValue TimeoutUnit … If Timeout is omitted a server *should* assume an infinite timeout."
> "An intermediary *MAY* propagate the timeout (with appropriate adjustment for elapsed wall-clock time) on the upstream request."

## Code shape today

- `edgion-gateway/src/routes/grpc/grpc_timeout.rs` — `parse_grpc_timeout(&str) -> Option<Duration>` and `format_grpc_timeout(Duration) -> String`; accepts the 6 spec units (`H`, `M`, `S`, `m`, `u`, `n`) with a max of 8 decimal digits per the gRPC-over-HTTP/2 spec. Malformed inputs return `None` so the caller falls back to route-level timeout semantics. The public parser also guards against multi-byte UTF-8 suffixes via `is_char_boundary` before slicing — wire callers go through `HeaderValue::to_str()` which is ASCII-only, but the boundary check closes a public-API panic surface.
- `edgion-gateway/src/ctx.rs::RequestGrpcInfo::client_deadline: Option<Instant>` — `#[serde(skip)]`, accessed through `RequestInfo` methods and set once with checked addition.
- `edgion-gateway/src/ctx.rs::EdgionHttpContext::grpc_remaining()` / `grpc_deadline_exceeded()` — helpers used by peer-timeout configuration and the early-abort check.
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs::build_request_metadata` — parses the header for any `is_grpc_request == true` request after protocol detection.
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs::upstream_peer` — runs the deadline check parallel to `request_timeout_exceeded`; `EdgionErrorCode::GrpcClientDeadlineExceeded` is recorded and the response path selects HTTP 504.
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs::configure_peer_timeouts` — folds `grpc_remaining` into `effective_read_write_timeout` via `min()`, sharing the same `Duration::ZERO` guard as the existing `request_remaining` branch.
- The separate connection fold preserves zero remaining budgets, sets both
  phase and total TCP/TLS timeout options, and records `connect_timeout_source`.
  See [establishment bounds and attribution](../../routing/regression-guards/connect-failure-after-deadline-uses-timeout-outcome.md)
  for timer precision, non-retryable outcomes, and connector boundaries.
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs` — new `upstream_request_filter` hook that rewrites `grpc-timeout` on the upstream-bound `RequestHeader` to the formatted `grpc_remaining`.
- Retry-decision sites (`pg_fail_to_connect.rs`, `pg_error_while_proxy.rs`, `pg_upstream_response_filter.rs` status-code retry branch) treat `request_timeout_exceeded() || grpc_deadline_exceeded()` as the unified "deadline blown — do not retry" guard, preventing a fresh LB pick / circuit-breaker counter increment after the caller has given up.
- If connection establishment fails after a deadline has expired,
  `pg_fail_to_connect.rs` records the canonical deadline diagnostic and returns
  a non-retryable HTTP 504 while retaining the original connection error as its
  binary-log cause. This produces gRPC `DEADLINE_EXCEEDED` instead of allowing a
  non-timeout connect error to map to another status. The route request deadline
  wins attribution when both it and `grpc-timeout` are expired.
- `pg_upstream_peer.rs` retry backoff sleep bounds the delay by `min(request_remaining, grpc_remaining)` and re-checks both deadlines after the sleep. Without this guard a tight `grpc-timeout` plus a coarse retry backoff (`grpc-timeout: 5S` + `retry.backoff: 10s` is the canonical case) would let the gateway sleep past the client's deadline and then open a fresh upstream connection nobody is waiting for.
- `pg_upstream_request_filter.rs` writes `grpc-timeout: 1n` to the upstream when `grpc_remaining()` is exactly zero — the rare race between `upstream_peer` returning OK and Pingora invoking the filter (connect / TLS handshake) where the deadline decays to zero. Without this, the upstream would see the stale original client value (e.g. `5S`) and run the full duration the caller has already abandoned.
- `edgion-tests/src/client/suites/grpc_route/basic/basic.rs::test_grpc_client_deadline_exceeded` — wire-level scenario: client sends `set_timeout(1ns)` and asserts `tonic::Code::DeadlineExceeded` arrives via the gateway.
- `edgion-tests/src/client/suites/grpc_route/timeout_rewrite_wire.rs` — standalone local-stack wire coverage. A fixed 300 ms request plugin proves the first upstream header is adjusted; a trace-keyed raw-H2 script sends `REFUSED_STREAM` for exactly the first two requests and a valid unary gRPC response on the third, proving exact attempt count, timeout grammar, monotonic nonincrease, and backoff-correlated decay. The same scripted response proves the gRPC-Web bridge preserves its data-plus-trailer framing. The recorder retains only the parsed numeric value and unit, never raw request metadata.
- The exact-zero `1n` case remains unit-level coverage. Its integration trigger would require scheduling expiry in the narrow, nondeterministic interval after peer selection but before the upstream request filter, so it is intentionally not asserted by the static wire suite.

## Coverage scope

Applies to **both** `is_grpc_request` paths:

- Pure gRPC over HTTP/2 (content-type `application/grpc`) — main GRPCRoute path.
- gRPC-Web (content-type `application/grpc-web` or `application/grpc-web+proto`) — even though the upstream may not be a real gRPC server, bounding gateway-side I/O is still desirable.

Non-gRPC traffic is untouched.

### Known limitation: streaming RPCs

Pingora's `peer.options.read_timeout` / `write_timeout` are **per-I/O-call**, not
absolute deadlines. A streaming RPC producing a chunk before each timeout
window can outlast `grpc-timeout`. The TCP/TLS total connection bound also
excludes offload dispatch, pool acquisition and later HTTP/2 session setup.
These are enforcement limits, not evidence that an absolute-deadline defect
has been fixed or accepted. A future finding must establish the affected phase,
required contract, and current call-chain evidence separately.

## Why this shape

1. **SHOULD/MAY clauses with no design conflict.** Gateway API conformance (GATEWAY-GRPC Core 13/13, ALL PASS) does not require this behavior but also does not preclude it; the fix is additive.
2. **Operator timeout knob preserved.** Route-level `request_timeout` (Gateway API field) still acts as the upper bound; client `grpc-timeout` only tightens it further when shorter. No opt-out flag was added.
3. **Hot-path overhead is bounded.** Parse only runs once per gRPC request (`grpc_client_deadline.is_none()` guard); `grpc_remaining()` is a single `Instant` arithmetic; the upstream header rewrite is a single `insert_header`.
4. **No `tracing::*` on the data-plane hot path.** Diagnostic surfacing goes through `EdgionErrorCode::GrpcClientDeadlineExceeded` + the access log, never through `tracing` macros (see `skills/03-coding/01-log-safety.md`).

## Re-evaluation triggers

- Pingora exposes a native gRPC deadline mechanism — Edgion's parser/rewriter then becomes redundant.
- gRPC spec changes the timeout format or adds a unit.
- An operator-reported incident shows the unconditional honoring breaks a legitimate workload (then evaluate adding a per-route or global opt-out annotation).

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a2.md → V3` (CONFIRM)
- Architecture: `skills/01-architecture/02-gateway/03-routes/02-grpc-route.md`
- Mapping table: `edgion-gateway/src/runtime/server/error_response.rs::http_to_grpc_status` (`504 → (4, "deadline exceeded")`)

## Resolution references

- `h2-grpc-03` — 2026-05-18.
- `bound-http-upstream-connect-by-end-to-end-deadlines` — added TCP/TLS bounds;
  current verification is maintained in the linked routing guard. The older
  verification and wire scenarios above are historical evidence, not fresh runs.
