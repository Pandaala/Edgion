# Gateway Integration Test Summary

## Execution Report

**Run date:** `2026-01-13`  
**Document scope:** Static Gateway suites for `ListenerHostname`, `AllowedRoutes`, and `Combined`  
**Pass rate:** `100%` within the scope of this report

## New Suite Details

### 1. `Gateway_ListenerHostname`

**Scenario count:** `5`  
**Pass rate:** `100%`  
**Ports:** `31240-31242`

| Test Case | Scenario | Result |
|-----------|----------|--------|
| `exact_hostname_match` | Exact hostname match (`api.example.com`) | `PASS` |
| `hostname_mismatch` | Hostname mismatch (negative test) | `PASS` |
| `wildcard_hostname_match` | Wildcard hostname match (`*.wildcard.example.com`) | `PASS` |
| `wildcard_root_mismatch` | Wildcard does not match the root domain (negative test) | `PASS` |
| `no_hostname_restriction` | Listener without hostname restriction | `PASS` |

Key checks:
- Exact listener hostname matching works.
- Wildcard matching works for subdomains.
- Wildcards correctly reject the root domain.
- Listeners without hostname restrictions accept any compatible hostname.

### 2. `Gateway_AllowedRoutes_Same`

**Scenario count:** `2`  
**Pass rate:** `100%`  
**Port:** `31210`

| Test Case | Scenario | Result |
|-----------|----------|--------|
| `same_namespace_allowed` | Same-namespace route is allowed | `PASS` |
| `diff_namespace_denied` | Cross-namespace route is rejected | `PASS` |

Key checks:
- `allowedRoutes.namespaces.from: Same` correctly limits access by namespace.
- Cross-namespace traffic is rejected with `404`.

### 3. `Gateway_AllowedRoutes_All`

**Scenario count:** `2`  
**Pass rate:** `100%`  
**Port:** `31211`

| Test Case | Scenario | Result |
|-----------|----------|--------|
| `all_same_namespace_allowed` | Same-namespace route is allowed | `PASS` |
| `all_cross_namespace_allowed` | Cross-namespace route is allowed | `PASS` |

Key checks:
- `allowedRoutes.namespaces.from: All` allows every namespace.
- Cross-namespace traffic works as expected.

### 4. `Gateway_AllowedRoutes_Kinds`

**Scenario count:** `2`  
**Pass rate:** `100%`  
**Port:** `31213`

| Test Case | Scenario | Result |
|-----------|----------|--------|
| `http_route_allowed` | `HTTPRoute` is allowed by `kinds` | `PASS` |
| `grpc_route_denied` | `GRPCRoute` is rejected by `kinds` | `PASS` |

Key checks:
- `allowedRoutes.kinds` correctly limits route types.
- Unsupported route kinds are rejected with `404`.

### 5. `Gateway_AllowedRoutes_Selector`

**Scenario count:** `2`  
**Pass rate:** `100%`  
**Port:** `31276`

| Test Case | Scenario | Result |
|-----------|----------|--------|
| `selector_same_namespace_allowed` | Route in a namespace with matching labels is allowed | `PASS` |
| `selector_cross_namespace_denied` | Cross-namespace route without a matching selector is rejected | `PASS` |

Key checks:
- `allowedRoutes.namespaces.from: Selector` works correctly.
- Matching namespace labels allow access.
- Non-matching cross-namespace routes are rejected with `404`.

### 6. `Gateway_Combined`

**Scenario count:** `5`  
**Pass rate:** `100%`  
**Ports:** `31230-31232`

| Test Case | Scenario | Result |
|-----------|----------|--------|
| `hostname_and_same_ns_match` | Hostname match plus same namespace | `PASS` |
| `hostname_match_diff_ns` | Hostname matches but namespace differs (negative test) | `PASS` |
| `same_ns_hostname_mismatch` | Same namespace but hostname mismatch (negative test) | `PASS` |
| `section_and_hostname_match` | `sectionName` and hostname both match | `PASS` |
| `section_match_hostname_mismatch` | `sectionName` matches but hostname does not (negative test) | `PASS` |

Key checks:
- Multiple listener constraints combine correctly.
- Any failed constraint results in a rejection.
- `sectionName` and hostname rules work together as expected.

## Coverage Summary

### Feature Matrix

| Feature | Positive Coverage | Negative Coverage | Combined Coverage | Status |
|---------|-------------------|-------------------|-------------------|--------|
| Listener hostname exact match | Yes | Yes | No | Complete |
| Listener hostname wildcard match | Yes | Yes | No | Complete |
| Listener without hostname | Yes | No | No | Complete |
| AllowedRoutes Same Namespace | Yes | Yes | Yes | Complete |
| AllowedRoutes All Namespaces | Yes | No | No | Complete |
| AllowedRoutes Selector | Yes | Yes | No | Complete |
| AllowedRoutes Kinds | Yes | Yes | No | Complete |
| `sectionName` + hostname | Yes | Yes | Yes | Complete |
| Hostname + AllowedRoutes | Yes | Yes (2 cases) | Yes | Complete |

### Added Test Assets

- New suites: `6`
- New test cases: `18`
- New configuration files: `18`
- New test code files: `10`

### Gateway API Specification Coverage

These tests validate the following Gateway API behaviors:

- `ParentReference.sectionName` binding
- `Listener.hostname` constraints
- `AllowedRoutes.namespaces.from` for `Same`, `All`, and `Selector`
- `AllowedRoutes.kinds` route-type filtering
- Combined constraint logic
- Default namespace behavior when `parentRef.namespace` is omitted

## Technical Notes

### Configuration Loading

1. Gateway manifests use the `01_Gateway.yaml` prefix to guarantee load order.
2. The suite waits 2 seconds after loading to allow dependent resources to settle.
3. Test ports avoid conflicts with existing suites.

### Coverage Focus

1. Hostname constraint behavior for exact and wildcard matching.
2. Namespace isolation for cross-namespace access control.
3. Route-type restrictions across different route kinds.
4. Interactions between multiple listener constraints.

## Suggested Next Steps

### Optional Extensions

- Add dynamic update coverage for Gateway hot-reload scenarios.
- Add multi-Gateway and multi-parentRef scenarios.

### Already Covered Implicitly

- Default `parentRef.namespace`
- Routes without `hostnames`
- Listeners without `hostname` in `no_hostname_restriction`

## References

- [Kubernetes Gateway API Specification](https://gateway-api.sigs.k8s.io/)
- Implementation: [`src/core/gateway/runtime/matching/route.rs`](../../../src/core/gateway/runtime/matching/route.rs)
- Test framework: [`examples/code/client/framework.rs`](../../code/client/framework.rs)
