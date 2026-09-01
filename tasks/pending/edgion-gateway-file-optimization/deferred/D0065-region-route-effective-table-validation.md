# D0065 — Define Complete RegionRoute Table Validation

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0099, RegionRoute analysis
- Category: configuration-semantics
- Canonical task: [`tasks/todo/define-region-route-effective-table-validation.md`](../../../todo/define-region-route-effective-table-validation.md)

## Evidence
RegionRoute base and overlay tables currently lack one canonical validator for endpoint syntax, hash-range bounds and overlap, empty-base/overrideRef relationships, and dangling, self-referential, or cyclic failover graphs. Documentation and runtime fallback behavior do not define every case consistently.

## Why deferred
Tightening any one rule changes operator admission and affects resources, Controller/Gateway preparse, overlay generations, management APIs, failover execution, and both documentation sets. Those semantics require an explicit design decision and one shared effective-table validator rather than opportunistic checks in the request path.

## Integration-test impact
No integration issue is created yet. The future design should first exhaust deterministic validator, preparse-parity, overlay-generation, and runtime table tests; if it intentionally changes a wire-visible hot-reload or failover contract, create a separate integration issue then.
