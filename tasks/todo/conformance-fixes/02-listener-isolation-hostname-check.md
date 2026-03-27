# Listener Isolation Hostname Check

## Priority: High | Effort: Small | Tests Fixed: 1 (12 subtests)

## Affected Tests
- GatewayHTTPListenerIsolation (12 subtests failed out of 24)

Failing subtests (same 6 fail in both sub-scenarios, totaling 12):

| Host | Path | Expected | Got | Issue |
|------|------|----------|-----|-------|
| `bar.example.com` | `/empty-hostname` | 404 | 200 | Empty-hostname route serving `*.example.com` traffic |
| `bar.foo.example.com` | `/empty-hostname` | 404 | 200 | Empty-hostname route serving `*.foo.example.com` traffic |
| `abc.foo.example.com` | `/empty-hostname` | 404 | 200 | Empty-hostname route serving exact-match traffic |
| `bar.foo.example.com` | `/wildcard-example-com` | 404 | 200 | `*.example.com` route serving `*.foo.example.com` traffic |
| `abc.foo.example.com` | `/wildcard-example-com` | 404 | 200 | `*.example.com` route serving exact-match traffic |
| `abc.foo.example.com` | `/wildcard-foo-example-com` | 404 | 200 | `*.foo.example.com` route serving exact-match traffic |

## Root Cause

The `sectionName` fast path in `check_gateway_listener_match` bypasses hostname isolation.

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

When a route's parentRef includes `sectionName`, the code performs a direct lookup and immediately returns **without verifying that the request hostname matches the listener's hostname constraint**. This means:

- The `attaches-to-empty-hostname` route (sectionName=`empty-hostname`) matches the empty-hostname listener and serves ALL hostnames via the catch-all domain bucket.
- The `attaches-to-wildcard-example-com` route serves `abc.foo.example.com` even though the `abc-foo-example-com` exact listener is more specific.

The hostname isolation logic (lines 66-82) only runs in the "slow path" when no sectionName is specified. Since all routes in this test use sectionName, isolation is never enforced.

## Code Locations
- `src/core/gateway/runtime/matching/route.rs:59-64` — sectionName fast path that skips hostname validation
- `src/core/gateway/runtime/matching/route.rs:66-82` — slow path with correct hostname isolation logic

## Proposed Fix

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
3. If no listener matches after all parentRefs are tried, the route returns `None` → 404.

## Risk Assessment

Low. The fix adds hostname validation to an existing code path. The slow path (no sectionName) already implements this logic correctly. The fix applies the same validation to the fast path.

**Regression concern:** Routes with sectionName that previously matched incorrectly will now correctly return 404. This is the desired behavior per Gateway API spec but could affect existing users relying on the current (incorrect) behavior.

## Benefit

Passes 12 subtests. Implements correct Gateway API listener isolation semantics — a fundamental spec requirement.

## Verification
```bash
./scripts/orbstack-dev.sh rebuild g
./scripts/orbstack-dev.sh test --run-test GatewayHTTPListenerIsolation
```
