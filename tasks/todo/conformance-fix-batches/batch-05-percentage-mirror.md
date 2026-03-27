# Batch 5: PercentageMirror Field

## Status: [ ] Not Started

## Tests to Fix
- HTTPRouteRequestPercentageMirror (currently: FAIL individually -- subtests 0 and 2 fail; subtest 1 passes)

## Problem

The `percentage` field of the `RequestMirror` filter (Gateway API v1.5 feature) is not supported -- only `fraction` is. The `HTTPRequestMirrorFilter` struct in `http_route.rs` has no `percentage` field. When the conformance test sets `percentage: 100`, the field is ignored (no matching Rust struct field), and `fraction` is `None`, which means "mirror all" per `should_mirror()` logic. This causes 100% mirroring instead of the requested percentage.

- Subtest 0 (`/percent-mirror`): `percentage: 100`, expected ~100 mirrored out of 500, got 503 (all mirrored).
- Subtest 1 (`/percent-mirror-fraction`): Uses `fraction` -- PASSES.
- Subtest 2 (`/percent-mirror-and-modify-headers`): Expected ~175 mirrored, got 500 (all mirrored).

## Fix Steps

### Step 1: Add `percentage` field to `HTTPRequestMirrorFilter`

**File:** `src/types/resources/http_route.rs`

```rust
pub struct HTTPRequestMirrorFilter {
    pub backend_ref: BackendObjectReference,
    pub fraction: Option<Fraction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentage: Option<i32>,
    // ... existing fields
}
```

### Step 2: Update `should_mirror()` to handle percentage

**File:** `src/core/gateway/plugins/http/request_mirror/plugin.rs` around line 68

```rust
fn should_mirror(&self) -> bool {
    // fraction takes precedence over percentage (per Gateway API spec)
    if let Some(fraction) = &self.config.fraction {
        // existing fraction logic unchanged
        let numerator = fraction.numerator;
        let denominator = fraction.denominator.unwrap_or(100);
        if denominator == 0 { return false; }
        if numerator >= denominator { return true; }
        let mut rng = rand::rng();
        let sample: i32 = rng.random_range(0..denominator);
        sample < numerator
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

### Step 3: Ensure controller maps the `percentage` field

**File:** Controller HTTP route handler (search for `RequestMirror` or `mirror` in controller handler code)

Verify the controller extracts `percentage` from the Kubernetes CRD resource and includes it in the synced config sent to the gateway. If the controller uses a direct serde deserialize from the K8s API object, the new field in the struct may be sufficient. If it manually maps fields, add the `percentage` mapping.

## Build & Deploy
```bash
# Both controller and gateway
./scripts/orbstack-dev.sh rebuild c g
```

## Verify
```bash
# Run individual test(s)
./scripts/orbstack-dev.sh test --run-test HTTPRouteRequestPercentageMirror

# After all tests in this batch pass individually, run core batch:
./scripts/orbstack-dev.sh test --core
```

## Result
- [ ] Individual tests pass
- [ ] Core batch score improved
- Previous score: __/60
- New score: __/60
