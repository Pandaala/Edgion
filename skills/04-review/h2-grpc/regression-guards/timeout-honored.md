---
name: timeout-honored
description: gRPC client `grpc-timeout` header is parsed, bounds upstream I/O, and is re-emitted with adjusted remaining time. Reject re-reports claiming the gateway still ignores it.
knowledge_type: regression-guards
---

# h2-grpc-03 — `grpc-timeout` header is honored end-to-end

**Status:** ✅ fixed · 2026-05-18
**Spec:** gRPC-over-HTTP/2 § Request → Timeout (SHOULD for server enforcement, MAY for intermediary propagation)

## Conclusion

Edgion now parses the client's `grpc-timeout` header at request entry, anchors a `client_deadline_at = start_time + parsed_duration` on the per-request `RequestInfo`, bounds the upstream `peer.options.read/write_timeout` to `min(backend_timeout, route_request_remaining, grpc_remaining)`, and re-emits an adjusted `grpc-timeout` header on the upstream request so downstream services see the correct remaining deadline.

When the parsed deadline has already elapsed before peer selection (in `pg_upstream_peer::upstream_peer`), the gateway aborts and the client sees DEADLINE_EXCEEDED (`grpc-status: 4`) via the existing 504 → grpc-status 4 mapping.

Suggestions of the form "the gateway still ignores `grpc-timeout`", "no upstream propagation", or "add an opt-out flag because operators don't want client deadlines bounding gateway behavior" are **not accepted** — the first two are no longer true, and the route-level `request_timeout` already provides the operator-facing knob.

## Spec clause

> "Timeout — grpc-timeout — TimeoutValue TimeoutUnit … If Timeout is omitted a server *should* assume an infinite timeout."
> "An intermediary *MAY* propagate the timeout (with appropriate adjustment for elapsed wall-clock time) on the upstream request."

## Code shape today

- `edgion-gateway/src/routes/grpc/grpc_timeout.rs` — `parse_grpc_timeout(&str) -> Option<Duration>` and `format_grpc_timeout(Duration) -> String`; accepts the 6 spec units (`H`, `M`, `S`, `m`, `u`, `n`) with a max of 8 decimal digits per the gRPC-over-HTTP/2 spec. Malformed inputs return `None` so the caller falls back to route-level timeout semantics. The public parser also guards against multi-byte UTF-8 suffixes via `is_char_boundary` before slicing — wire callers go through `HeaderValue::to_str()` which is ASCII-only, but the boundary check closes a public-API panic surface.
- `edgion-gateway/src/ctx.rs::RequestInfo::grpc_client_deadline: Option<Instant>` — `#[serde(skip)]`, set once per request from `start_time + parsed_value`.
- `edgion-gateway/src/ctx.rs::EdgionHttpContext::grpc_remaining()` / `grpc_deadline_exceeded()` — helpers used by peer-timeout configuration and the early-abort check.
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs::build_request_metadata` — parses the header for any `is_grpc_request == true` request after protocol detection.
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs::upstream_peer` — runs the deadline check parallel to `request_timeout_exceeded`; `EdgionErrorCode::GrpcClientDeadlineExceeded` is recorded and the response path selects HTTP 504.
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs::configure_peer_timeouts` — folds `grpc_remaining` into `effective_read_write_timeout` via `min()`, sharing the same `Duration::ZERO` guard as the existing `request_remaining` branch.
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

## Coverage scope

Applies to **both** `is_grpc_request` paths:

- Pure gRPC over HTTP/2 (content-type `application/grpc`) — main GRPCRoute path.
- gRPC-Web (content-type `application/grpc-web` or `application/grpc-web+proto`) — even though the upstream may not be a real gRPC server, bounding gateway-side I/O is still desirable.

Non-gRPC traffic is untouched.

### Known limitation: streaming RPCs

Pingora's `peer.options.read_timeout` / `write_timeout` are **per-I/O-call**, not absolute deadlines. A long-running server-streaming RPC that produces a chunk before every per-read timeout window expires will run past the overall `grpc-timeout` value — the deadline is consulted at peer selection, retry decisions, and per-chunk-read boundaries, but not as a single absolute kill switch across the whole stream. This matches the existing route-level `request_timeout` semantics. If end-to-end streaming-deadline enforcement is needed in the future, the natural extension point is `pg_upstream_response_body_filter.rs` (and the symmetric request-body filter) — short-circuit when `ctx.grpc_deadline_exceeded()` returns true. Current behavior is documented here so future reviewers do not re-open this as a separate finding.

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
