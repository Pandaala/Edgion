# Timeout Zero Handling

## Priority: Medium | Effort: Medium | Tests Fixed: 2

## Affected Tests
- HTTPRouteTimeoutRequest (subtest 1: expected 504, got 200; subtest 2: expected 200, got 500)
- HTTPRouteTimeoutBackendRequest (subtest 2: expected 200, got 504)

## Root Cause

Two related issues:

### Issue 1: `"0s"` parsed as zero instead of "disable timeout"

The Gateway API spec states that `"0s"` means "no timeout" (disable timeout), not "zero seconds". The `ParsedRouteTimeouts::from_config` method parses `"0s"` into `Duration::ZERO` via `parse_duration`. A zero-duration timeout is then applied literally:

- For `backendRequest`: `Duration::ZERO` is applied to `peer.options.{connection,read,write}_timeout`, causing every request to immediately time out with 504.
- For `request`: `Duration::ZERO` is applied as an actual 0-second deadline, causing immediate 500 errors.

### Issue 2: `request_timeout` not enforced end-to-end

The code in `pg_upstream_peer.rs` checks `request_timeout` only to prevent starting a new retry attempt after the deadline, and uses `backend_request_timeout` to set `peer.options.{connection,read,write}_timeout`. There is no mechanism to enforce `request_timeout` as a hard ceiling on the entire request-response transaction (including backend processing time).

This causes subtest 1 of HTTPRouteTimeoutRequest to fail: the route sets `timeouts.request` to a value shorter than 1s, but the backend's 1s delay completes successfully (expected 504, got 200).

## Code Locations
- `ParsedRouteTimeouts::from_config` — parses `"0s"` into `Duration::ZERO` (exact file TBD in timeout/route config parsing)
- `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` — `request_timeout` only used to skip retry attempts, not as end-to-end deadline; `configure_peer_timeouts()` applies `backend_request_timeout` to Pingora peer options

## Proposed Fix

### Fix 1: Handle `"0s"` as "disable timeout" (Trivial)

In `ParsedRouteTimeouts::from_config`, after parsing, convert `Duration::ZERO` to `None`:

```rust
let request_timeout = config.request.as_ref().and_then(|s| {
    parse_duration(s).ok().and_then(|d| if d.is_zero() { None } else { Some(d) })
});
let backend_request_timeout = config.backend_request.as_ref().and_then(|s| {
    parse_duration(s).ok().and_then(|d| if d.is_zero() { None } else { Some(d) })
});
```

When `backend_request_timeout` is `None`, `configure_peer_timeouts` falls through to the global default — correct for "no per-route override".

### Fix 2: Enforce `request_timeout` end-to-end (Medium)

Wrap the upstream proxy cycle in a `tokio::time::timeout(request_timeout)` to enforce the end-to-end deadline. This could be done in `upstream_request_filter` or by setting a deadline on the Pingora session. Requires careful integration with Pingora's proxy lifecycle and retry logic.

## Risk Assessment

- **Fix 1 (`0s` handling):** Low risk. A 2-line change. The `backendRequest` timeout is fully implemented and applied to Pingora peer options; the only bug is the `0s` handling.
- **Fix 2 (end-to-end request timeout):** Medium risk. Requires careful integration with Pingora's proxy lifecycle and retry logic. Could introduce unexpected behavior if the timeout fires mid-response.

## Benefit

- Fix 1 alone fixes HTTPRouteTimeoutBackendRequest subtest 2 and HTTPRouteTimeoutRequest subtest 2.
- Fix 2 additionally fixes HTTPRouteTimeoutRequest subtest 1.
- These are Extended-feature tests required for full HTTP conformance. Timeout is a fundamental Gateway API feature.

## Verification
```bash
./scripts/orbstack-dev.sh rebuild g
./scripts/orbstack-dev.sh test --run-test HTTPRouteTimeoutRequest
./scripts/orbstack-dev.sh test --run-test HTTPRouteTimeoutBackendRequest
```
