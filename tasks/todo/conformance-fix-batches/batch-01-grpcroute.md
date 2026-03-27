# Batch 1: GRPCRoute Error Response

## Status: [ ] Not Started

## Tests to Fix
- GRPCExactMethodMatching (currently: FAIL individually -- 1 of 3 subtests)
- GRPCRouteHeaderMatching (currently: FAIL individually -- all 11 subtests)
- GRPCRouteListenerHostnameMatching (currently: FAIL individually -- all 8 subtests)
- GRPCRouteWeight (currently: FAIL individually -- 1 subtest)

## Problem

Two issues cause all 4 failures:

1. **`end_response_grpc_unimplemented` produces RST_STREAM instead of gRPC trailers-only response.** The function writes HTTP 200 with `grpc-status: 12` headers, then calls `session.shutdown().await`. Pingora's HTTP/2 layer converts the shutdown into RST_STREAM with INTERNAL_ERROR, corrupting the clean trailers-only response. Clients see `code = Internal (13)` instead of `code = Unimplemented (12)`.

2. **`upstream_peer_grpc` returns Pingora InternalError on missing backend.** When `handle_grpc_upstream` returns `Ok(None)` or `Err`, the code calls `end_response_500` and returns `Err(PingoraError::new(ErrorType::InternalError))`. Pingora translates this into RST_STREAM. For gRPC, this should return properly formatted gRPC errors.

## Fix Steps

### Step 1: Remove `session.shutdown()` from `end_response_grpc_unimplemented`

**File:** `src/core/gateway/runtime/server/error_response.rs` lines 134-152

Remove the `session.shutdown().await` call after `write_response_header(resp_box, true)`. The `end_of_stream=true` flag on the HEADERS frame is sufficient to produce a proper gRPC trailers-only response. The explicit shutdown causes RST_STREAM.

```rust
// BEFORE (broken):
pub async fn end_response_grpc_unimplemented(session: &mut Session, ...) -> Result<()> {
    let mut resp = ResponseHeader::build(200, None)?;
    resp.insert_header("grpc-status", "12")?;
    resp.insert_header("grpc-message", "...")?;
    resp.insert_header("content-type", "application/grpc")?;
    session.write_response_header(Box::new(resp), true).await?;
    session.shutdown().await;  // <-- REMOVE THIS LINE
    Ok(())
}
```

If removing `shutdown()` alone is not enough (Pingora still sends RST_STREAM), try sending an empty DATA frame before the trailers:
```rust
session.write_response_header(Box::new(resp), false).await?;
session.write_response_body(Some(Bytes::new()), true).await?;
```

### Step 2: Fix `upstream_peer_grpc` error handling

**File:** `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` lines 84-126

When `handle_grpc_upstream` returns `Ok(None)` (no backend found), instead of calling `end_response_500` and returning `Err(InternalError)`, call `end_response_grpc_unimplemented` (after Fix 1) and return a non-error that stops the proxy pipeline:

```rust
// BEFORE:
Ok(None) => {
    end_response_500(session).await;
    return Err(PingoraError::new(ErrorType::InternalError));
}

// AFTER:
Ok(None) => {
    end_response_grpc_unimplemented(session, "no matching gRPC backend").await?;
    return Err(PingoraError::new(ErrorType::HTTPStatus(200)));
    // Or use whatever Pingora idiom stops the proxy without sending RST_STREAM
}
```

### Step 3: Verify controller GRPCRoute sync (if tests still fail)

Check `src/core/gateway/routes/grpc/conf_handler_impl.rs` to ensure GRPCRoute manifests for header matching, listener hostname matching, and weighted backends are synced before the conformance test sends RPCs.

## Build & Deploy
```bash
# Gateway only (both fixes are in gateway code)
./scripts/orbstack-dev.sh rebuild g
```

## Verify
```bash
# Run individual test(s)
./scripts/orbstack-dev.sh test --run-test GRPCExactMethodMatching
./scripts/orbstack-dev.sh test --run-test GRPCRouteHeaderMatching
./scripts/orbstack-dev.sh test --run-test GRPCRouteListenerHostnameMatching
./scripts/orbstack-dev.sh test --run-test GRPCRouteWeight

# After all tests in this batch pass individually, run core batch:
./scripts/orbstack-dev.sh test --core
```

## Result
- [ ] Individual tests pass
- [ ] Core batch score improved
- Previous score: __/60
- New score: __/60
