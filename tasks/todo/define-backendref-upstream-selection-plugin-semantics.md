# Define BackendRef Upstream-selection Plugin Semantics

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | design |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define whether backendRef-scoped request plugins may select a different upstream,
then enforce that choice consistently in validation and runtime dispatch.

## Requirements

- Inventory DynamicExternalUpstream, RegionRoute, DirectEndpoint, DynamicInternalUpstream, Canary, and other jump-producing request plugins at backendRef scope.
- Choose between rejecting upstream-selection plugins at backendRef scope or supporting a bounded late-selection lifecycle.
- If supported, define recursion prevention, selected backend/plugin ownership, URI/timeout state, retry identity, and canonical access-log attribution.
- Enforce the same rule in Controller/schema validation and Gateway runtime preparation.
- Add focused unit and later integration coverage for the chosen contract.

## Out of scope

- A local second jump-dispatch that leaves recursion or ownership unspecified.
- Changing ordinary rule/Gateway/global upstream-selection behavior.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Schedule a separate repair | The correction requires a product choice and coordinated validation/runtime semantics beyond a local URI-order correction. |
