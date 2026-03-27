# Listener Isolation Conformance Test Failures

## Failed Tests

1. **GatewayHTTPListenerIsolation** (12 subtests failed, 12 passed)
2. **HTTPRouteListenerPortMatching** (4 subtests failed, 1 passed)

---

## Test 1: GatewayHTTPListenerIsolation

### Test Setup

The test creates a Gateway `http-listener-isolation` with four HTTP listeners on port 80, each with a different hostname:

| Listener | Hostname |
|----------|----------|
| `empty-hostname` | *(none / catch-all)* |
| `wildcard-example-com` | `*.example.com` |
| `wildcard-foo-example-com` | `*.foo.example.com` |
| `abc-foo-example-com` | `abc.foo.example.com` |

Four HTTPRoutes are created, each attached to one listener via `parentRef.sectionName`, routing to `infra-backend-v1`.

The test sends requests with various Host headers and path combinations, expecting **listener isolation**: a route attached to listener X should only serve requests whose hostname matches listener X's hostname constraint, and should NOT serve requests that match a more-specific listener Y, even if listener X also matches.

### Failing Subtests (all timeout at 30s)

| # | Host | Path | Expected | Got | Issue |
|---|------|------|----------|-----|-------|
| 4 | `bar.example.com` | `/empty-hostname` | 404 | 200 | Empty-hostname route serving `*.example.com` traffic |
| 8 | `bar.foo.example.com` | `/empty-hostname` | 404 | 200 | Empty-hostname route serving `*.foo.example.com` traffic |
| 12 | `abc.foo.example.com` | `/empty-hostname` | 404 | 200 | Empty-hostname route serving exact-match traffic |
| 9 | `bar.foo.example.com` | `/wildcard-example-com` | 404 | 200 | `*.example.com` route serving `*.foo.example.com` traffic |
| 13 | `abc.foo.example.com` | `/wildcard-example-com` | 404 | 200 | `*.example.com` route serving exact-match traffic |
| 14 | `abc.foo.example.com` | `/wildcard-foo-example-com` | 404 | 200 | `*.foo.example.com` route serving exact-match traffic |

The same 6 subtests fail in both sub-scenarios (`hostnames_are_configured_only_in_listeners` and `intersecting_hostnames_are_configured_in_listeners_and_HTTPRoutes`), totaling 12 failures.

### Root Cause

**The `sectionName` fast path in `check_gateway_listener_match` bypasses hostname isolation.**

In `src/core/gateway/runtime/matching/route.rs` lines 59-64:

```rust
// Fast path: sectionName → O(1) two-level HashMap lookup
if let Some(section_name) = &pr.section_name {
    if let Some(entry) = index.get(&gateway_key, section_name) {
        return Some(entry.info.clone());
    }
    continue;
}
```

When a route's parentRef includes `sectionName`, the code performs a direct lookup and immediately returns without verifying that the **request hostname** actually matches the **listener's hostname constraint**. This means:

- The `attaches-to-empty-hostname` route (sectionName=`empty-hostname`) matches the empty-hostname listener entry in the index. Since the fast path never checks whether a more-specific listener should take priority, this route serves ALL hostnames via the catch-all domain bucket.
- Similarly, the `attaches-to-wildcard-example-com` route serves `abc.foo.example.com` even though the `abc-foo-example-com` exact listener is more specific.

The hostname isolation logic (lines 66-82) only runs in the "slow path" when no sectionName is specified. Since all routes in this test use sectionName, isolation is never enforced.

### Proposed Fix

Add hostname validation to the sectionName fast path:

```rust
if let Some(section_name) = &pr.section_name {
    if let Some(entry) = index.get(&gateway_key, section_name) {
        // Enforce listener isolation even with sectionName
        if let Some(ref listener_host) = entry.hostname {
            if !hostname_matches_listener(request_hostname, listener_host) {
                continue;
            }
        } else {
            // Empty-hostname listener: check if a more-specific listener matches
            let Some(gw) = index.get_gateway(&gateway_key) else {
                return Some(entry.info.clone());
            };
            if gw.has_more_specific(request_hostname, None) {
                continue;
            }
        }
        return Some(entry.info.clone());
    }
    continue;
}
```

This ensures:
1. If the listener has a hostname, the request hostname must match it.
2. If the listener has no hostname (catch-all), a more-specific listener must not exist for the request hostname.
3. If no listener matches after all parentRefs are tried, the route returns `None` and the request falls through to 404.

---

## Test 2: HTTPRouteListenerPortMatching

### Test Setup

The test creates a Gateway `httproute-listener-port-matching` with three listeners:

| Listener | Port | Hostname |
|----------|------|----------|
| `listener-80` | 80 | `foo.com` |
| `listener-8080` | 8080 | *(none)* |
| `listener-8090` | 8090 | `foo.com` |

Three HTTPRoutes:
- `backend-v1`: attached to `listener-80`, serves `foo.com` on port 80
- `backend-v2`: attached to `listener-8080`, serves `foo.com` and `bar.com` on port 8080
- `backend-v3`: attached to `listener-8090`, serves `foo.com` on port 8090

### Failing Subtests

| # | Request | Expected | Error |
|---|---------|----------|-------|
| 1 | `foo.com:8080/` | `infra-backend-v2` | `unexpected port in gateway address: 192.168.194.243:8080` |
| 2 | `bar.com:8080/` | `infra-backend-v2` | `unexpected port in gateway address: 192.168.194.243:8080` |
| 3 | `foo.com:8090/` | `infra-backend-v3` | `unexpected port in gateway address: 192.168.194.243:8090` |
| 4 | `bar.com:8090/` | empty (no match) | `unexpected port in gateway address: 192.168.194.243:8090` |

Subtest 0 (`foo.com/` on port 80) passes.

### Root Cause

**Edgion does not support dynamic listener creation. Ports 8080 and 8090 are never bound.**

The test dynamically creates a Gateway with listeners on ports 8080 and 8090. Edgion's Pingora-based architecture only binds listener ports at startup based on the Gateway resources present at that time (see PLAN.md: "Listener dynamic creation: Workaround — Pre-create Gateway before gateway pod starts"). Since the `httproute-listener-port-matching` Gateway is created after the gateway pod is running, ports 8080 and 8090 are never opened.

The conformance test framework's `roundtripper` detects that `gateway.status.addresses` only advertises port 80 and rejects requests to other ports with `unexpected port in gateway address`. All port-8080 and port-8090 requests fail immediately and retry for 30s until timeout.

### Is Dynamic Listener Creation the Blocker?

**Yes.** This test is fundamentally blocked by the lack of runtime listener creation. The gateway process must be able to:

1. Detect new Gateway resources with new port listeners
2. Bind new TCP listeners at runtime
3. Advertise the new ports in `gateway.status.addresses`

This is listed as **P4** in PLAN.md ("Dynamic listener modification — requires architectural change in the gateway to add/remove listeners at runtime without restart").

### Proposed Fix

No software fix is feasible without architectural changes to Pingora's listener lifecycle. Options:

1. **Pre-create workaround**: Add the `httproute-listener-port-matching` Gateway with ports 8080/8090 to `manifests/conformance-gateway.yaml` and restart the gateway pod before the test. This mirrors the existing workaround for the main conformance gateway but is fragile and doesn't scale to all conformance tests.

2. **Dynamic listener support** (P4): Implement runtime port binding in `src/core/gateway/runtime/server/base.rs`. This requires Pingora API support for adding listeners to a running server, which may not exist.

3. **Skip the test**: Add `HTTPRouteParentRefPort` to the exempt features list.

---

## Summary

| Test | Root Cause | Fix Type | Complexity | Benefit |
|------|-----------|----------|------------|---------|
| GatewayHTTPListenerIsolation | sectionName fast path bypasses hostname isolation in `check_gateway_listener_match` | Code fix in `route.rs` | **Low** — ~15 lines changed in one function, existing `has_more_specific` logic is correct | Passes 12 subtests; correct Gateway API listener isolation semantics |
| HTTPRouteListenerPortMatching | Dynamic listener creation not supported; ports 8080/8090 never bound | Architecture change (P4) or pre-create workaround | **High** for real fix; **Low** for workaround | Passes 4 subtests; required for full HTTPRoute conformance |

## Risk Assessment

### GatewayHTTPListenerIsolation Fix

- **Risk**: Low. The fix adds hostname validation to an existing code path. The slow path (no sectionName) already implements this logic correctly. The fix applies the same validation to the fast path.
- **Regression concern**: Routes with sectionName that previously matched incorrectly will now correctly return 404. This is the desired behavior per Gateway API spec but could affect existing users relying on the current (incorrect) behavior.
- **Testing**: Add unit tests for `check_gateway_listener_match` with sectionName + hostname mismatch. Run full conformance suite.

### HTTPRouteListenerPortMatching Fix

- **Risk**: High for dynamic listener support (new Pingora integration, potential stability issues). Low for pre-create workaround (just YAML + restart script changes).
- **Regression concern**: Dynamic listener creation could introduce port conflicts, resource leaks, or startup race conditions.

## Recommendation

1. **GatewayHTTPListenerIsolation**: **Fix now.** Low complexity, low risk, high value. The bug is a clear logic gap where the sectionName fast path skips hostname validation. The fix is straightforward and aligns with Gateway API spec semantics. Source file: `src/core/gateway/runtime/matching/route.rs`.

2. **HTTPRouteListenerPortMatching**: **Defer.** The real fix (dynamic listener creation) is a P4 architectural change that affects Pingora integration and is out of scope for conformance-focused work. The pre-create workaround is fragile and only helps this specific test. Recommend adding `HTTPRouteParentRefPort` to exempt features or skip list until P4 is addressed. If the workaround approach is acceptable, it can be done quickly by adding ports 8080/8090 to `manifests/conformance-gateway.yaml`.
