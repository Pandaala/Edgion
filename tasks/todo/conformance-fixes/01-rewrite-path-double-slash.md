# RewritePath Double-Slash Bug

## Priority: High | Effort: Trivial | Tests Fixed: 1

## Affected Tests
- HTTPRouteRewritePath (subtest 1: `/strip-prefix/three`)

## Root Cause

The `ReplacePrefixMatch` logic in `url_rewrite.rs` (line 56) concatenates replacement and suffix without handling the edge case where both end/start with `/`:

```rust
format!("{}{}", replacement, &original_path[len..])
```

When the route match prefix is `/strip-prefix` and the replacement is `/` (prefix stripping):
- `replacement` = `"/"`
- `original_path[len..]` = `/three` (remainder after removing `/strip-prefix`)
- Result: `"/" + "/three"` = `"//three"`

The `/strip-prefix` subtest (without trailing path) works because `original_path[len..]` is empty, yielding just `"/"`.

## Code Locations
- `src/core/gateway/plugins/runtime/gateway_api_filters/url_rewrite.rs:56` — `ReplacePrefixMatch` path joining logic

## Proposed Fix

After computing the rewritten path in the `ReplacePrefixMatch` branch, handle the double-slash edge case:

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

Or more generally: if `replacement` ends with `/` and `suffix` starts with `/`, trim one slash:

```rust
let suffix = &original_path[len..];
if replacement.ends_with('/') && suffix.starts_with('/') {
    format!("{}{}", replacement, &suffix[1..])
} else {
    format!("{}{}", replacement, suffix)
}
```

## Risk Assessment

Low. A 3-4 line conditional in `url_rewrite.rs`. The Gateway API spec explicitly defines the path-joining semantics for `ReplacePrefixMatch`, and the upstream conformance test suite codifies the expected behavior. No impact on other rewrite paths.

## Benefit

Fixes 1 conformance subtest. Path rewrite is a commonly-used feature; this double-slash bug could affect real-world users using prefix stripping with replacement `/`.

## Verification
```bash
./scripts/orbstack-dev.sh rebuild g
./scripts/orbstack-dev.sh test --run-test HTTPRouteRewritePath
```
