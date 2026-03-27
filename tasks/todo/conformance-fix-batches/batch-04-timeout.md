# Batch 4: Timeout "0s" Semantics

## Status: [ ] Not Started

## Tests to Fix
- HTTPRouteTimeoutRequest (currently: FAIL individually -- subtest 1: expected 504 got 200; subtest 2: expected 200 got 500)
- HTTPRouteTimeoutBackendRequest (currently: FAIL individually -- subtest 2: expected 200 got 504)

## Problem

Two related issues:

1. **`"0s"` parsed as zero duration instead of "disable timeout".** The Gateway API spec states `"0s"` means "no timeout" (disable). `ParsedRouteTimeouts::from_config` parses `"0s"` into `Duration::ZERO` via `parse_duration`. A zero-duration timeout is then applied literally: for `backendRequest`, `Duration::ZERO` sets `peer.options.{connection,read,write}_timeout` to 0, causing immediate 504. For `request`, `Duration::ZERO` causes immediate 500.

2. **`request_timeout` not enforced end-to-end.** The code in `pg_upstream_peer.rs` only checks `request_timeout` to prevent starting new retry attempts after the deadline. There is no mechanism to enforce it as a hard ceiling on the full request-response transaction. This causes subtest 1 to fail: route sets `timeouts.request` shorter than 1s, but backend's 1s delay completes (expected 504, got 200).

## Fix Steps

### Step 1: Handle `"0s"` as "disable timeout"

**File:** `ParsedRouteTimeouts::from_config` (find via `rg "ParsedRouteTimeouts" --type rust`)

After parsing duration strings, convert `Duration::ZERO` to `None`:

```rust
let request_timeout = config.request.as_ref().and_then(|s| {
    parse_duration(s).ok().and_then(|d| if d.is_zero() { None } else { Some(d) })
});
let backend_request_timeout = config.backend_request.as_ref().and_then(|s| {
    parse_duration(s).ok().and_then(|d| if d.is_zero() { None } else { Some(d) })
});
```

When `backend_request_timeout` is `None`, `configure_peer_timeouts` falls through to the global default -- correct for "no per-route override".

This alone fixes HTTPRouteTimeoutBackendRequest subtest 2 and HTTPRouteTimeoutRequest subtest 2.

### Step 2: Enforce `request_timeout` end-to-end

**File:** `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs`

Wrap the upstream proxy cycle in `tokio::time::timeout(request_timeout)`:

```rust
if let Some(deadline) = request_timeout {
    match tokio::time::timeout(deadline, proxy_to_upstream(session, peer)).await {
        Ok(result) => result,
        Err(_elapsed) => {
            // Return 504 Gateway Timeout
            end_response_504(session).await;
            return Err(...);
        }
    }
}
```

Alternatively, set a deadline on the Pingora session or use `upstream_request_filter` to inject a timeout. This requires careful integration with Pingora's proxy lifecycle.

This fixes HTTPRouteTimeoutRequest subtest 1.

## Build & Deploy
```bash
# Gateway only
./scripts/orbstack-dev.sh rebuild g
```

## Verify
```bash
# Run individual test(s)
./scripts/orbstack-dev.sh test --run-test HTTPRouteTimeoutRequest
./scripts/orbstack-dev.sh test --run-test HTTPRouteTimeoutBackendRequest

# After all tests in this batch pass individually, run core batch:
./scripts/orbstack-dev.sh test --core
```

## Result
- [ ] Individual tests pass
- [ ] Core batch score improved
- Previous score: __/60
- New score: __/60
