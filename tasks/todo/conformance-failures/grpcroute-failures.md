# GRPCRoute Data-Plane Failures — Root Cause Analysis

## Summary

4 GRPCRoute conformance tests fail. The failures split into two distinct issues:

| Test | Total Subtests | Pass | Fail | Failure Mode |
|------|---------------|------|------|-------------|
| GRPCExactMethodMatching | 3 | 2 | 1 | Wrong gRPC status code (Internal/13 instead of Unimplemented/12) |
| GRPCRouteHeaderMatching | 11 | 0 | 11 | RST_STREAM INTERNAL_ERROR on every request |
| GRPCRouteListenerHostnameMatching | 8 | 0 | 8 | RST_STREAM INTERNAL_ERROR on every request |
| GRPCRouteWeight | 1 | 0 | 1 | RST_STREAM INTERNAL_ERROR on every request |

## Issue 1: `end_response_grpc_unimplemented` Produces RST_STREAM Instead of gRPC Unimplemented

**Affects:** GRPCExactMethodMatching subtest 2 (EchoThree), and likely all "should receive Unimplemented" subtests in the other three tests.

### Evidence

From the log, ExactMethodMatching subtest 2 repeatedly gets:
```
RPC finished with error: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: INTERNAL_ERROR
expected status code to be Unimplemented (12), but got Internal (13)
```

Meanwhile subtests 0 and 1 (which match actual routes) succeed with correct responses:
```
RPC finished with response assertions:{fully_qualified_method:"/gateway_api_conformance.echo_basic.grpcecho.GrpcEcho/Echo" ... pod:"grpc-infra-backend-v1-..."}
Request passed
```

### Root Cause

The `end_response_grpc_unimplemented` function at `src/core/gateway/runtime/server/error_response.rs:134-152` writes an HTTP 200 response with `grpc-status: 12` and `grpc-message` in the initial response headers, then calls `session.shutdown().await`. However, Pingora's HTTP/2 layer appears to convert this into an RST_STREAM with INTERNAL_ERROR instead of properly sending a gRPC trailers-only response.

In the gRPC protocol (over HTTP/2), an unmatched request should receive a "Trailers-Only" response: a single HEADERS frame with END_STREAM containing both the HTTP status and the `grpc-status` trailer. The current code calls `write_response_header(resp_box, true)` (end_of_stream=true), which should produce the correct frame. However, calling `session.shutdown().await` immediately afterward may trigger Pingora to send an additional RST_STREAM or GOAWAY, corrupting the clean trailers-only response.

**Alternatively**, the issue may be that the gateway falls through to HTTP route matching after gRPC matching fails (line 43-56 in `pg_request_filter.rs`), and the HTTP 404 path may interact incorrectly with the HTTP/2 stream if there's a race between the response write and Pingora's internal error handling when `request_filter` returns `Ok(true)`.

### Code Locations

| File | Line(s) | Role |
|------|---------|------|
| `src/core/gateway/runtime/server/error_response.rs` | 134-152 | `end_response_grpc_unimplemented` — writes gRPC status 12 response |
| `src/core/gateway/runtime/server/error_response.rs` | 101-130 | `end_response_404` — dispatches to grpc_unimplemented for gRPC requests |
| `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs` | 31-56 | Route matching flow: gRPC match → HTTP fallback → 404 |

## Issue 2: RST_STREAM INTERNAL_ERROR on ALL Requests for HeaderMatching, ListenerHostnameMatching, Weight

**Affects:** GRPCRouteHeaderMatching (all 11 subtests), GRPCRouteListenerHostnameMatching (all 8 subtests), GRPCRouteWeight (1 subtest). Total: 20 subtests.

### Evidence

Every single RPC to these three tests gets `RST_STREAM INTERNAL_ERROR` within 2-8ms of being sent:
```
grpc.go:279: Sending RPC
grpc.go:279: RPC finished with error: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: INTERNAL_ERROR
```

This happens for requests that SHOULD match routes (e.g., HeaderMatching subtest 0 expects to route to grpc-infra-backend-v1) as well as requests expecting Unimplemented. The fast RST_STREAM proves the TCP connection reaches the gateway — the gateway actively rejects the HTTP/2 stream.

Key observation: GRPCExactMethodMatching runs FIRST and its matching subtests succeed. The three failing tests run AFTER ExactMethodMatching. Each test applies its own GRPCRoute manifest.

### Root Cause Hypothesis: Pingora HTTP/2 Connection Corruption

The most likely cause is that the failing `end_response_grpc_unimplemented` from ExactMethodMatching subtest 2 corrupts the HTTP/2 connection state. The conformance suite's gRPC client (in `gateway-api/conformance@v1.5.1/utils/grpc/grpc.go`) reuses connections across subtests within the same test case:

```go
func (c *DefaultClient) ensureConnection(address string, req *RequestMetadata) error {
    if c.Conn != nil {
        return nil  // reuse existing connection
    }
    // ...
    c.Conn, err = grpc.NewClient(address, dialOpts...)
}
```

After RST_STREAM, the client calls `resetConnection()` and creates a new connection. However, on the **gateway side**, if `session.shutdown()` in `end_response_grpc_unimplemented` sent a GOAWAY frame on the HTTP/2 connection, subsequent connections from the same client could be affected (port-forward connection reuse, or the gateway's listener state).

**Alternative hypothesis: Port-forward HTTP/2 multiplexing failure.** The test uses `kubectl port-forward` which tunnels through SPDY. After 30 seconds of repeated RST_STREAM errors from ExactMethodMatching subtest 2, the port-forward tunnel may enter a degraded state where all new HTTP/2 connections fail. The three subsequent tests all start immediately after ExactMethodMatching's 30s timeout and all exhibit identical RST_STREAM behavior.

**Alternative hypothesis: gRPC client connects directly to cluster IP.** The conformance suite's gRPC client uses `grpc.NewClient(address)` where address is the Gateway's cluster IP (`192.168.194.243:80`). The `CustomDialContext` on the `DefaultRoundTripper` only applies to HTTP tests, NOT to the gRPC client. On OrbStack, cluster IPs are directly reachable from the host, which explains why ExactMethodMatching works. But after ExactMethodMatching's repeated failures, something may cause subsequent connections to fail. This hypothesis is weakened by the fact that RST_STREAM responses arrive in 2ms (proving connectivity), but a corrupted gateway-side state could explain it.

**Most likely root cause: The gateway sends RST_STREAM because the gRPC route was not found, but the error response mechanism is broken.** When the gRPC route match fails AND the HTTP route match also fails, the call path is:
1. `try_match_grpc_route` → fails (returns false)
2. `!ctx.is_grpc_route_matched` → true → try HTTP route matching
3. HTTP route matching fails → `end_response_404` → `end_response_grpc_unimplemented`
4. Response write fails or `session.shutdown()` corrupts the HTTP/2 stream → RST_STREAM

This would explain why ALL requests in HeaderMatching/ListenerHostnameMatching/Weight fail: the GRPCRoute manifests from these tests may not match correctly (controller processing lag, or the match engine fails for these route configurations), causing ALL requests to hit the broken error path.

### Code Locations

| File | Line(s) | Role |
|------|---------|------|
| `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs` | 31-56 | gRPC→HTTP fallback route matching |
| `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` | 90-101 | `upstream_peer_grpc` — returns `Err(InternalError)` on missing backend, causes RST_STREAM |
| `src/core/gateway/runtime/server/error_response.rs` | 134-152 | `end_response_grpc_unimplemented` — may trigger RST_STREAM |
| `src/core/gateway/routes/grpc/match_engine.rs` | 215-318 | `GrpcMatchEngine::match_route` — service/method based matching |
| `src/core/gateway/routes/grpc/match_unit.rs` | 208-247 | `deep_match` — hostname + gateway/listener + header verification |
| `src/core/gateway/routes/grpc/integration.rs` | 43-63 | `try_match_grpc_route` — entry point for gRPC matching |
| `src/core/gateway/routes/grpc/conf_handler_impl.rs` | all | GRPCRoute config handler — syncs routes from controller |

## Proposed Fix

### Fix 1: Fix `end_response_grpc_unimplemented` (Priority: HIGH)

The response function must produce a valid gRPC trailers-only response that Pingora can send as a clean HTTP/2 HEADERS frame with END_STREAM. Steps:

1. **Remove `session.shutdown().await`** after writing the response header with `end_of_stream=true`. The END_STREAM flag on the HEADERS frame is sufficient; the explicit shutdown may cause Pingora to send RST_STREAM.
2. **Test that Pingora's `write_response_header(header, true)` actually emits a single HEADERS frame with END_STREAM** — verify this doesn't trigger additional frames.
3. If Pingora cannot send a proper trailers-only response, consider sending HTTP 200 + empty DATA frame + HEADERS (trailers) with `grpc-status: 12`.

**File:** `src/core/gateway/runtime/server/error_response.rs` lines 134-152

### Fix 2: Fix `upstream_peer_grpc` Error Handling (Priority: HIGH)

When `handle_grpc_upstream` returns `Ok(None)` or `Err`, the current code calls `end_response_500` and returns `Err(PingoraError::new(ErrorType::InternalError))`. Pingora translates this error into RST_STREAM. For gRPC requests, this should return gRPC-formatted errors:
- `Ok(None)` (no backend) → gRPC `Unavailable (14)` or `Unimplemented (12)`
- `Err` (error selecting backend) → gRPC `Internal (13)` but properly formatted

**File:** `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` lines 84-126

### Fix 3: Investigate Controller GRPCRoute Sync Timing (Priority: MEDIUM)

Verify that the controller correctly processes GRPCRoute manifests for header matching, listener hostname matching, and weighted backends. Check that:
- The gateway receives updated route tables BEFORE the conformance test starts sending RPCs
- The `conf_handler_impl.rs` correctly parses all GRPCRoute fields (headers, hostnames, weights)

**File:** `src/core/gateway/routes/grpc/conf_handler_impl.rs`

### Fix 4: Add `CustomDialContext` for gRPC Client (Priority: LOW)

The conformance test's gRPC client may bypass port-forward. If the gateway is reachable via cluster IP (OrbStack), this is not a blocker. But for portability, consider implementing a custom `grpc.Client` that uses the same dial context. This requires overriding the `GRPCClient` field in the conformance suite options if the API supports it.

**File:** `examples/gateway-api-conformance/conformance_test.go`

## Risk Assessment

| Fix | Risk | Effort |
|-----|------|--------|
| Fix 1 (unimplemented response) | Low — isolated to error response path, no impact on happy path | Small (< 1 hour) |
| Fix 2 (upstream_peer_grpc errors) | Low — only affects gRPC error paths | Small (< 1 hour) |
| Fix 3 (controller sync timing) | Medium — may reveal deeper sync issues | Medium (investigation) |
| Fix 4 (test dial context) | Low — test-only change | Small |

## Benefit

- **4 tests, 23 subtests** would potentially pass
- GRPCExactMethodMatching: Fix 1 alone would make all 3 subtests pass (2 already pass)
- HeaderMatching/ListenerHostnameMatching/Weight: Requires Fix 1+2, potentially Fix 3
- Passing all 4 core GRPCRoute tests enables GATEWAY-GRPC conformance profile certification

## Recommendation

**Fix** (not skip/defer). The gRPC routing data plane is already implemented and works for the happy path (ExactMethodMatching subtests 0,1 pass). The failures are in error handling paths (Fixes 1-2) and possibly route sync timing (Fix 3). The effort is small relative to the benefit of GATEWAY-GRPC conformance.

**Recommended order:**
1. Fix 1 — `end_response_grpc_unimplemented` (remove `session.shutdown()`, verify trailers-only response)
2. Fix 2 — `upstream_peer_grpc` error paths (return gRPC-formatted errors instead of RST_STREAM)
3. Re-run conformance tests to see if Fixes 1-2 resolve all failures
4. If HeaderMatching/ListenerHostnameMatching/Weight still fail, investigate Fix 3 (controller sync)
5. Fix 4 is optional, only needed if running outside OrbStack
