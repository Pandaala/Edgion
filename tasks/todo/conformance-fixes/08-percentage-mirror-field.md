# Percentage Mirror Field

## Priority: Medium | Effort: Small | Tests Fixed: 1

## Affected Tests
- HTTPRouteRequestPercentageMirror (subtests 0 and 2 fail; subtest 1 passes)

## Root Cause

The `percentage` field of the `RequestMirror` filter (Gateway API v1.5 feature `HTTPRouteRequestPercentageMirror`) is not supported — only `fraction` is.

The `HTTPRequestMirrorFilter` struct in `http_route.rs` only has a `fraction` field; there is no `percentage` field. When the conformance test sets `percentage: 100` in the filter, it gets ignored (no matching field in the Rust struct) and the gateway mirrors **100% of requests** (because `fraction` is `None`, which means "mirror all" per line 68-69 of `plugin.rs`).

Evidence:
- Subtest 0 (`/percent-mirror`): Test sends 500 requests with `percentage: 100`. Expected ~100 mirrored, got 503 (all mirrored because percentage field ignored).
- Subtest 1 (`/percent-mirror-fraction`): Uses `fraction` (numerator/denominator) — **PASSES** because `fraction` is correctly implemented.
- Subtest 2 (`/percent-mirror-and-modify-headers`): Expected ~175 mirrored, got 500 (same issue as subtest 0).

## Code Locations
- `src/types/resources/http_route.rs` — `HTTPRequestMirrorFilter` struct, missing `percentage` field
- `src/core/gateway/plugins/http/request_mirror/plugin.rs:68-69` — `should_mirror()` treats `None` fraction as "mirror all"
- Controller HTTP route handler — must extract the Gateway API CRD `percentage` field and include it in the synced config

## Proposed Fix

### 1. Add `percentage` field to the struct

In `src/types/resources/http_route.rs`:
```rust
pub struct HTTPRequestMirrorFilter {
    pub backend_ref: BackendObjectReference,
    pub fraction: Option<Fraction>,
    /// Percentage of requests to mirror (0-100). Gateway API v1.5 feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentage: Option<i32>,
    // ... existing fields
}
```

### 2. Update `should_mirror()` to handle percentage

In `src/core/gateway/plugins/http/request_mirror/plugin.rs`:
```rust
fn should_mirror(&self) -> bool {
    // fraction takes precedence over percentage
    if let Some(fraction) = &self.config.fraction {
        // existing fraction logic
    } else if let Some(pct) = self.config.percentage {
        if pct <= 0 { return false; }
        if pct >= 100 { return true; }
        let mut rng = rand::rng();
        let sample: i32 = rng.random_range(0..100);
        sample < pct
    } else {
        true // no fraction or percentage = mirror all
    }
}
```

### 3. Ensure controller maps the field

Verify the controller's HTTP route handler extracts the `percentage` field from the Kubernetes CRD resource and includes it in the synced config sent to the gateway.

## Risk Assessment

Medium. The data-plane change is straightforward. The controller-side mapping of the Gateway API CRD `percentage` field to the internal struct needs verification — the controller's HTTP route handler must extract the `percentage` field from the Kubernetes resource and include it in the synced config.

## Benefit

Fixes 1 conformance test (2 subtests). This is a v1.5 Gateway API feature (`HTTPRouteRequestPercentageMirror`). The GatewayClass already advertises this feature as supported, so this is a correctness issue. The `fraction`-based mirror works fine; `percentage` is a newer convenience API.

## Verification
```bash
./scripts/orbstack-dev.sh rebuild c g
./scripts/orbstack-dev.sh test --run-test HTTPRouteRequestPercentageMirror
```
