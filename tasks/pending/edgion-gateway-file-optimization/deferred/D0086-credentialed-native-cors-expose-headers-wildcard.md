# D0086 — Decide Credentialed Native CORS Expose-Headers Wildcard Semantics

## Meta

- Status: needs-discussion
- Severity: medium
- Discovered in: B0124 Gateway API CORS filter analysis
- Category: protocol-conformance
- Canonical task: [`tasks/todo/decide-credentialed-native-cors-expose-headers-wildcard.md`](../../../todo/decide-credentialed-native-cors-expose-headers-wildcard.md)

## Evidence

The runtime emits configured `exposeHeaders` unchanged, so `allowCredentials: true` with `exposeHeaders: ["*"]` produces `Access-Control-Expose-Headers: *`. The vendored Gateway API v1.5 contract says a credentialed response cannot use that wildcard even though the current CRD admits the combination. Unlike methods or allowed headers, there is no request header from which the exposed response-header list can be reflected.

## Why deferred

Rejecting the configuration, omitting the header, treating the wildcard literally, or deriving an explicit response-header list have different conformance and compatibility consequences. The repository needs a version-pinned contract decision before changing validation or wire behavior.

## Integration-test impact

The canonical task owns focused validation/runtime tests and review of official conformance coverage. It should add cross-process coverage if the selected policy changes publication or response behavior; no separate integration issue is required now.
