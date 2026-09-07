# D0081 — Fail Closed on Plugin IP Matcher Limit Violations

## Meta

- Status: needs-investigation
- Severity: medium
- Discovered in: B0121 shared condition evaluation final review
- Category: plugin-publication
- Canonical task: [`tasks/todo/fail-closed-plugin-ip-matcher-limit-violations.md`](../../../todo/fail-closed-plugin-ip-matcher-limit-violations.md)

## Evidence

`compile_plugins` rejects a resource whose aggregate referenced IP rules exceed `MAX_REFERENCED_IP_RULES` only by appending a `preparse_errors` diagnostic. The error does not enter `PluginRuntime::invalid_plugins`, so `is_activatable` can remain true and `seal_fail_closed` can publish an otherwise runnable generation even though the Controller reports `Accepted=False` and still synchronizes the resource.

## Why deferred

This is a resource-level aggregate limit rather than an individual `PluginConditions::compile` or `validate` failure. Its invalid-generation identity, update policy, and reject-gate tests need an independent publication audit instead of expanding B0121's entry-level condition correction.

## Integration-test impact

Existing integration-test files remain frozen. The later task must first add focused publication tests and create a separate integration-coverage issue only if a genuinely necessary scenario cannot be added without modifying an existing integration file.
