# D0043 — Enforce Exactly One CtxSet Transform Operation

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0071, CtxSet audit
- Category: resource-validation
- Depends on: B0071
- Canonical task: [`tasks/todo/enforce-one-ctx-set-transform-operation.md`](../../../todo/enforce-one-ctx-set-transform-operation.md)

## Evidence
`TransformConfig` documents that at most one transform operation may be configured, but shared resource validation currently accepts multiple fields. Gateway runtime then silently selects the first operation in the established `replace`, `substring`, `case`, `prefix`, `suffix`, `trim` priority order.

## Why deferred
Correcting the admission contract spans shared resource validation, schema-facing documentation, and resource tests. B0071 intentionally preserves current runtime priority while removing request-path allocations, so an operator-visible validation change does not belong in that behavior-preserving batch.

## Integration-test impact
No new integration issue is required. The future implementation should be proven at shared resource validation and plugin compilation boundaries without changing existing integration tests.
