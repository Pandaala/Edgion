# B0264 — HTTP DSL Error-policy Tests

## Scope

- Pipeline state: complete; target-only implementation, independent final review, and coordinator gates passed.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `b6e3dcb7`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/error_policy.rs`.
- Target blob: `a77ab64d939616bec423e6e9e88de09a68386120`.
- Read scope includes the complete test file, every DSL error-policy application and dye/result mapping path, adjacent error/decision tests, resource validation/defaults, and current review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, target-only unit coverage.
- Add one Ignore-policy regression that naturally panics inside a mocked `req.header` accessor and proves `catch_unwind` produces `GoodNext`, exactly one FailOpen `script-error` dye outcome, and only the static `err: vm panic` log.
- Add one BodyAccess regression under Ignore that proves the fixed generic HTTP 500, exactly one FailClose `request-body-access-error` outcome, and only the static `err: request body access error` log.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/tests/error_policy.rs` only. A local helper may return both result and PluginLog; existing policy/dye coverage remains unchanged.
- Preserve all policy/default/config/runtime behavior, BodyAccess hard fail-close, static safe logging, shared request/response execution, and frozen integration tests. No new integration issue is required.

## Implementation and Review

- Added a naturally triggered VM panic regression under Ignore and asserted the exact GoodNext, FailOpen `script-error`, and static-only log behavior.
- Added a BodyAccess regression under Ignore and asserted the fixed generic HTTP 500, FailClose `request-body-access-error`, and static-only log behavior.
- Updated the local helper to return the PluginLog without weakening the three existing policy/dye assertions.
- Independent final review: LGTM with no Must Fix or Should Fix. It independently traced both tests through the production `execute_safe` branches and confirmed exact result, dye, static-log, helper, and scope invariants.

## Verification

- Implementer: target 9/9 and full Gateway DSL 333/333 passed; Gateway all-target check, format, diff, English-only, and Gateway no-tracing checks passed.
- Reviewer: target 9/9 and full Gateway DSL 333/333 passed; format, diff, and frozen-path checks passed.
- Coordinator full workspace checks passed: format, locked workspace all-target check and Clippy, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration guards.
- Frozen integration manifest: 3,231 paths unchanged.
- Commit: `420b0905` (`Test DSL hard error boundaries`); no push.
