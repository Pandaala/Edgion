# Conformance Test Failures: Timeout, RewritePath, PercentageMirror

---

## 1. HTTPRouteTimeoutRequest

### Failing subtests

- **Subtest 1** (`/request-timeout?delay=1s`): Expected `504`, got `200`. The route sets `timeouts.request` to a value shorter than 1s, but the gateway does not enforce the per-route request timeout — the backend's 1s delay completes successfully.
- **Subtest 2** (`/disable-request-timeout?delay=1s`): Expected `200`, got `500`. The route sets `timeouts.request: "0s"` to disable the timeout. The gateway treats `0s` as an invalid/zero duration instead of "no timeout", returning 500.

### Root cause

**Per-route `request` timeout is not enforced on the end-to-end request duration.** The code in `pg_upstream_peer.rs` checks `request_timeout` only to prevent starting a new retry attempt after the deadline, and uses `backend_request_timeout` to set `peer.options.{connection,read,write}_timeout`. However, there is no mechanism to enforce `request_timeout` as a hard ceiling on the entire request-response transaction (including backend processing time).

Additionally, the `ParsedRouteTimeouts::from_config` method parses `"0s"` into `Duration::ZERO` via `parse_duration`. A zero-duration timeout is then applied as an actual 0-second deadline, causing immediate 500 errors instead of being interpreted as "disable timeout" per the Gateway API spec.

### Is the feature implemented?

Partially. The `HTTPRouteTimeouts` struct is parsed and stored. `backend_request_timeout` is applied to Pingora peer options (connect/read/write). However:
- `request_timeout` is only used to skip retry attempts, not as a real end-to-end timeout.
- `"0s"` (disable timeout) is not handled per Gateway API semantics.

### What's missing

1. A true end-to-end request timeout mechanism (e.g., wrapping the entire proxy cycle in `tokio::time::timeout` using `request_timeout`).
2. Special handling of `Duration::ZERO` → treat as "no timeout / infinite" rather than "0 seconds".

### Proposed fix

1. In `ParsedRouteTimeouts::from_config`, when the parsed duration is `Duration::ZERO`, store `None` instead (meaning "no timeout / disabled").
2. Wrap the upstream proxy cycle in a `tokio::time::timeout(request_timeout)` to enforce the end-to-end deadline. This could be done in `upstream_request_filter` or by setting a deadline on the Pingora session.

### Risk / complexity

Medium. The `0s` fix is trivial (a one-line check). The end-to-end `request_timeout` enforcement requires careful integration with Pingora's proxy lifecycle and retry logic.

### Benefit

Fixes 2 conformance subtests across 2 test cases (HTTPRouteTimeoutRequest + HTTPRouteTimeoutBackendRequest share the same `0s` issue). These are Extended-feature tests required for full HTTP conformance.

### Recommendation

**Priority: High.** Timeout is a fundamental Gateway API feature, and the `0s` bug also affects HTTPRouteTimeoutBackendRequest.

---

## 2. HTTPRouteTimeoutBackendRequest

### Failing subtest

- **Subtest 2** (`/disable-backend-timeout?delay=1s`): Expected `200`, got `504` repeatedly (30 attempts, 30s timeout). The route sets `timeouts.backendRequest: "0s"` to disable the backend timeout. The gateway applies `Duration::ZERO` as an actual 0-second timeout on the peer, causing every request to immediately time out with 504.

### Root cause

Same as HTTPRouteTimeoutRequest: `"0s"` is parsed to `Duration::ZERO` and applied literally to `peer.options.{connection,read,write}_timeout`. The Gateway API spec states that `"0s"` means "no timeout" (disable timeout), not "zero seconds".

### Is the feature implemented?

Yes, `backendRequest` timeout is fully implemented and applied to Pingora peer options in `configure_peer_timeouts()`. The only bug is the `0s` handling.

### What's missing

Special-casing `Duration::ZERO` as "no timeout" in `ParsedRouteTimeouts::from_config` (or in `configure_peer_timeouts` where the value is applied).

### Proposed fix

In `ParsedRouteTimeouts::from_config`, after parsing `backend_request` and `request`, convert `Duration::ZERO` to `None`:

```rust
let request_timeout = config.request.as_ref().and_then(|s| {
    parse_duration(s).ok().and_then(|d| if d.is_zero() { None } else { Some(d) })
});
let backend_request_timeout = config.backend_request.as_ref().and_then(|s| {
    parse_duration(s).ok().and_then(|d| if d.is_zero() { None } else { Some(d) })
});
```

When `backend_request_timeout` is `None`, `configure_peer_timeouts` falls through to the global default — which is correct for "no per-route override".

### Risk / complexity

Low. A 2-line change in `ParsedRouteTimeouts::from_config`.

### Benefit

Fixes 1 conformance subtest. Combined with the HTTPRouteTimeoutRequest fix (same root cause), this unblocks both timeout test cases.

### Recommendation

**Priority: High.** Trivial fix, shared root cause with HTTPRouteTimeoutRequest.

---

## 3. HTTPRouteRewritePath

### Failing subtest

- **Subtest 1** (`/strip-prefix/three`): Expected the backend to see path `/three`, but the backend received `//three` (double leading slash). This retried for 30s and never self-corrected.

### Root cause

The `ReplacePrefixMatch` logic in `url_rewrite.rs` (line 56) concatenates:

```rust
format!("{}{}", replacement, &original_path[len..])
```

When the route match prefix is `/strip-prefix` and the replacement is `/` (to strip the prefix), the calculation is:
- `replacement` = `"/"`
- `original_path[len..]` = `/three` (the remainder after removing `/strip-prefix`)
- Result: `"/" + "/three"` = `"//three"`

The code does not handle the edge case where `replacement` ends with `/` and the remaining suffix starts with `/`, producing a double-slash.

The `/strip-prefix` subtest (without trailing path) works because `original_path[len..]` is empty, yielding just `"/"`.

### Is the feature implemented?

Yes, fully implemented in `src/core/gateway/plugins/runtime/gateway_api_filters/url_rewrite.rs`. The logic is correct for most cases; this is an edge case bug in prefix-strip path joining.

### What's missing

Normalization of the joined path to collapse `//` into `/` when the replacement is `/` (or more generally, when `replacement` ends with `/` and the suffix starts with `/`).

### Proposed fix

After computing the rewritten path in the `ReplacePrefixMatch` branch, normalize double slashes:

```rust
if let Some(len) = matched_prefix_len {
    if len <= original_path.len() {
        let suffix = &original_path[len..];
        if replacement == "/" && suffix.starts_with('/') {
            suffix.to_string()
        } else {
            format!("{}{}", replacement, suffix)
        }
    } else {
        replacement
    }
}
```

Or more generally: if `replacement` ends with `/` and `suffix` starts with `/`, trim one slash.

### Risk / complexity

Low. A 3-4 line conditional in `url_rewrite.rs`. The Gateway API spec explicitly defines the path-joining semantics for `ReplacePrefixMatch`, and the upstream conformance test suite codifies the expected behavior.

### Benefit

Fixes 1 conformance subtest. Path rewrite is a commonly-used feature; this double-slash bug could affect real-world users using prefix stripping.

### Recommendation

**Priority: High.** Simple fix, high user impact, affects a commonly-used pattern (prefix stripping with replacement `/`).

---

## 4. HTTPRouteRequestPercentageMirror

### Failing subtests

- **Subtest 0** (`/percent-mirror`): Test sends 500 requests with `percentage: 100` (mirror 100% via the `percentage` field, not `fraction`). The test then checks the mirror backend's pod logs. Expected ~100 mirrored requests (from 500 * 100/500 or similar), but got **503** — far more than expected. After 5 retries, the test gave up.
- **Subtest 2** (`/percent-mirror-and-modify-headers`): Same pattern. Expected ~175 mirrored requests, got **500**. The mirror backend is receiving requests from ALL subtests cumulatively because pod logs are not isolated between subtests.

**Subtest 1** (`/percent-mirror-fraction`): PASSED. This subtest uses `fraction` (numerator/denominator), which Edgion correctly implements.

### Root cause

The key issue is that **the `percentage` field of the `RequestMirror` filter is not supported** — only `fraction` is. Looking at the Gateway API v1.5 spec for `HTTPRouteRequestPercentageMirror`, there are two ways to specify mirroring rate:

1. `fraction` — `{numerator, denominator}` (the traditional way)
2. `percentage` — a simple integer 0-100 (the new v1.5 way via the `HTTPRouteRequestPercentageMirror` feature)

The `HTTPRequestMirrorFilter` struct in `http_route.rs` only has a `fraction` field; there is no `percentage` field. When the conformance test sets `percentage: 100` in the filter, it gets ignored (no matching field in the Rust struct) and the gateway mirrors **100% of requests** (because `fraction` is `None`, which means "mirror all" per line 68-69 of `plugin.rs`).

The test then counts mirror logs in the backend pod and finds the count is cumulative across all test iterations (each retry sends 500 more requests, and the backend pod accumulates all mirror hits). The expected count is based on percentage, but actual is ~500 per iteration (all mirrored).

### Is the feature implemented?

`fraction`-based mirroring: **Yes**, fully working (subtest 1 passes).
`percentage`-based mirroring: **No**. The `percentage` field from Gateway API is not parsed or used.

### What's missing

1. Add a `percentage` field to `HTTPRequestMirrorFilter` in `http_route.rs`.
2. In `should_mirror()`, check `percentage` in addition to `fraction`. If `percentage` is set, convert it to fraction logic (e.g., `numerator = percentage, denominator = 100`).
3. Handle the case where both `fraction` and `percentage` are set (Gateway API spec likely says `fraction` takes precedence or they are mutually exclusive).

### Proposed fix

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

In `src/core/gateway/plugins/http/request_mirror/plugin.rs`, update `should_mirror()`:
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

Also need to ensure the controller maps the Gateway API `percentage` field to this struct field during HTTP route processing.

### Risk / complexity

Medium. The data-plane change is straightforward. The controller-side mapping of the Gateway API CRD `percentage` field to the internal struct needs verification — the controller's HTTP route handler must extract the `percentage` field from the Kubernetes resource and include it in the synced config.

### Benefit

Fixes 2 conformance subtests. This is a v1.5 Gateway API feature (`HTTPRouteRequestPercentageMirror`). Without it, the feature flag reported to GatewayClass is misleading (claimed supported but not functional).

### Recommendation

**Priority: Medium.** The `fraction`-based mirror works fine; `percentage` is a newer convenience API. However, the GatewayClass already advertises `HTTPRouteRequestPercentageMirror` as a supported feature, so this is a correctness issue.

---

## Summary

| Test | Root Cause | Fix Complexity | Priority |
|------|-----------|---------------|----------|
| HTTPRouteTimeoutRequest | `0s` not treated as "disable"; `request_timeout` not enforced end-to-end | Medium | High |
| HTTPRouteTimeoutBackendRequest | `0s` not treated as "disable" | Low | High |
| HTTPRouteRewritePath | Double-slash when prefix-strip replacement is `/` | Low | High |
| HTTPRouteRequestPercentageMirror | `percentage` field not parsed; only `fraction` is supported | Medium | Medium |

All four tests use the pre-created `same-namespace` Gateway on port 80 (via `192.168.194.243`). None create new Gateways, so the dynamic listener provisioning issue is not a factor here.
