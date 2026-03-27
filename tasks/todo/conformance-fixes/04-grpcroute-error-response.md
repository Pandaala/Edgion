# GRPCRoute Error Response

## Priority: High | Effort: Small | Tests Fixed: 4 (23 subtests)

## Affected Tests
- GRPCExactMethodMatching (1 of 3 subtests fail)
- GRPCRouteHeaderMatching (all 11 subtests fail)
- GRPCRouteListenerHostnameMatching (all 8 subtests fail)
- GRPCRouteWeight (1 subtest fails)

## Root Cause

Two distinct issues cause all failures:

### Issue 1: `end_response_grpc_unimplemented` produces RST_STREAM instead of gRPC trailers-only response

The function at `error_response.rs:134-152` writes an HTTP 200 response with `grpc-status: 12` and `grpc-message` in the initial response headers, then calls `session.shutdown().await`. Pingora's HTTP/2 layer converts this into an RST_STREAM with INTERNAL_ERROR instead of properly sending a gRPC trailers-only response.

In the gRPC protocol (over HTTP/2), an unmatched request should receive a "Trailers-Only" response: a single HEADERS frame with END_STREAM containing both the HTTP status and the `grpc-status` trailer. The code calls `write_response_header(resp_box, true)` (end_of_stream=true), which should produce the correct frame, but calling `session.shutdown().await` immediately afterward triggers Pingora to send an additional RST_STREAM or GOAWAY, corrupting the clean trailers-only response.

Evidence from logs:
```
expected status code to be Unimplemented (12), but got Internal (13)
RPC finished with error: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: INTERNAL_ERROR
```

### Issue 2: `upstream_peer_grpc` returns Pingora InternalError on missing backend

When `handle_grpc_upstream` returns `Ok(None)` or `Err`, the code at `pg_upstream_peer.rs:90-101` calls `end_response_500` and returns `Err(PingoraError::new(ErrorType::InternalError))`. Pingora translates this error into RST_STREAM. For gRPC requests, this should return gRPC-formatted errors instead.

This affects ALL requests in HeaderMatching/ListenerHostnameMatching/Weight — if routes aren't matched (due to controller sync timing), every request hits the broken error path producing RST_STREAM.

## Code Locations
- `src/core/gateway/runtime/server/error_response.rs:134-152` — `end_response_grpc_unimplemented` writes gRPC status 12 response then calls `session.shutdown()`
- `src/core/gateway/runtime/server/error_response.rs:101-130` — `end_response_404` dispatches to grpc_unimplemented for gRPC requests
- `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs:31-56` — route matching flow: gRPC match → HTTP fallback → 404
- `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs:84-126` — `upstream_peer_grpc` returns `Err(InternalError)` on missing backend, causes RST_STREAM
- `src/core/gateway/routes/grpc/match_engine.rs:215-318` — `GrpcMatchEngine::match_route` service/method based matching
- `src/core/gateway/routes/grpc/match_unit.rs:208-247` — `deep_match` hostname + gateway/listener + header verification
- `src/core/gateway/routes/grpc/integration.rs:43-63` — `try_match_grpc_route` entry point for gRPC matching
- `src/core/gateway/routes/grpc/conf_handler_impl.rs` — GRPCRoute config handler, syncs routes from controller

## Proposed Fix

### Fix 1: Fix `end_response_grpc_unimplemented` (Priority: HIGH)

1. **Remove `session.shutdown().await`** after writing the response header with `end_of_stream=true`. The END_STREAM flag on the HEADERS frame is sufficient; the explicit shutdown causes RST_STREAM.
2. Verify that Pingora's `write_response_header(header, true)` emits a single HEADERS frame with END_STREAM — no additional frames.
3. If Pingora cannot send a proper trailers-only response, consider sending HTTP 200 + empty DATA frame + HEADERS (trailers) with `grpc-status: 12`.

**File:** `src/core/gateway/runtime/server/error_response.rs` lines 134-152

### Fix 2: Fix `upstream_peer_grpc` error handling (Priority: HIGH)

When `handle_grpc_upstream` returns errors, return gRPC-formatted errors instead of Pingora InternalError:
- `Ok(None)` (no backend) → gRPC `Unavailable (14)` or `Unimplemented (12)`
- `Err` (error selecting backend) → gRPC `Internal (13)` but properly formatted as trailers-only response

**File:** `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` lines 84-126

### Fix 3: Investigate controller GRPCRoute sync timing (Priority: MEDIUM)

Verify that the controller correctly processes GRPCRoute manifests for header matching, listener hostname matching, and weighted backends. Check that the gateway receives updated route tables BEFORE the conformance test starts sending RPCs.

**File:** `src/core/gateway/routes/grpc/conf_handler_impl.rs`

## Risk Assessment

| Fix | Risk | Effort |
|-----|------|--------|
| Fix 1 (unimplemented response) | Low — isolated to error response path, no impact on happy path | Small (< 1 hour) |
| Fix 2 (upstream_peer_grpc errors) | Low — only affects gRPC error paths | Small (< 1 hour) |
| Fix 3 (controller sync timing) | Medium — may reveal deeper sync issues | Medium (investigation) |

## Benefit

4 tests, 23 subtests would potentially pass. GRPCExactMethodMatching subtests 0 and 1 already pass (happy path works). Passing all 4 core GRPCRoute tests enables GATEWAY-GRPC conformance profile certification.

**Recommended order:**
1. Fix 1 — remove `session.shutdown()`, verify trailers-only response
2. Fix 2 — return gRPC-formatted errors instead of RST_STREAM
3. Re-run conformance tests; if HeaderMatching/ListenerHostnameMatching/Weight still fail, investigate Fix 3

## Verification
```bash
./scripts/orbstack-dev.sh rebuild g
./scripts/orbstack-dev.sh test --run-test GRPCExactMethodMatching
./scripts/orbstack-dev.sh test --run-test GRPCRouteHeaderMatching
./scripts/orbstack-dev.sh test --run-test GRPCRouteListenerHostnameMatching
./scripts/orbstack-dev.sh test --run-test GRPCRouteWeight
```
