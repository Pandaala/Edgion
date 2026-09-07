# D0082 — Fail Closed on Invalid AiGuard/AiProxy Ordering

## Meta

- Status: needs-investigation
- Severity: high
- Discovered in: B0121 shared condition evaluation final review
- Category: plugin-publication
- Canonical task: [`tasks/todo/fail-closed-ai-guard-proxy-order-violations.md`](../../../todo/fail-closed-ai-guard-proxy-order-violations.md)

## Evidence

`compile_plugins` calls the shared `collect_ai_guard_ordering_errors` helper, but currently appends its result only to `preparse_errors`. The runtime invalid-plugin set can remain empty, so `is_activatable` stays true and `seal_fail_closed` does not install a reject gate even when an `AiGuard` placed after `AiProxy` cannot inspect the provider-bound prompt. The Controller reports the same error but still synchronizes the resource under lenient admission.

## Why deferred

This is a cross-entry ordering invariant, not an individual condition compile/validation result. Correctly associating a resource-level ordering failure with immutable publication and hot-update policy requires a focused security review and must not widen B0121.

## Integration-test impact

Existing integration-test files remain frozen. The later task should add focused publication tests first and create a separate integration-coverage issue if an end-to-end ordering scenario is still necessary.
