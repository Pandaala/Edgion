# B0261 — HTTP DSL Control-flow Tests

## Scope

- Pipeline state: complete; implementation, independent final review, and coordinator gates passed.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `87fb44df`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/control_flow.rs`.
- Target blob: `c7e09f3a63f536b1f1f3ae21f6f023ab9170140a`.
- Read scope includes the full test file, every exercised compiler/VM control-flow path, runtime/error semantics, adjacent tests, and current review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; moderate, mechanically identical compiler correction with focused runtime regressions; plan-first is not required.
- All three source-loop compilers currently backpatch `break` after `LoopEnd`. Because the VM pops its lexical loop counter only at `LoopEnd`, a broken inner loop leaks its counter on top of the outer counter. Repeating that pattern lets the outer loop evade `max_loop_iterations` until the wider step budget and grows the counter stack once per iteration.
- Capture each loop's `break_target` at its `LoopEnd` opcode, so natural exhaustion and `break` both execute exactly one cleanup while existing `continue` targets remain unchanged. Cover while, for-range, and for-in inner-loop breaks and require `LoopLimitExceeded`, never `StepLimitExceeded`.
- Exact technical allow-list: `edgion-dsl/src/compiler/loop_.rs`, `edgion-gateway/src/plugins/http/dsl/tests/control_flow.rs`, `skills/01-architecture/02-gateway/dsl-engine-security-model.md`, `skills/04-review/dsl/SKILL.md`, and new `skills/04-review/dsl/regression-guards/break-preserves-loop-counter-stack.md`.
- Preserve innermost-break semantics, continue targets, thresholds, return behavior, bytecode schema/version, typed errors, workspace DAG, observability, and frozen integration tests.

## Implementation and Review

- The three loop compilers now capture `break_target` at the `LoopEnd` opcode, so natural exhaustion and `break` both execute exactly one cleanup while continue targets remain unchanged.
- Added one table-driven Gateway regression covering immediately broken inner while, range-for, and list-for loops under a low loop limit and higher step limit.
- Updated the DSL security model and maintained review knowledge with lexical counter-stack ownership.
- Independent final review: LGTM with no Must Fix or Should Fix. The reviewer independently traced all three loop forms through compiler backpatching and VM cleanup, confirmed the regression fails on the old behavior, and verified the preserved continue/innermost-break/return/version invariants.
- Separate untrusted-bytecode CFG validation is recorded as D0134; it does not expand this source-compiler batch.

## Verification

- Implementer: control-flow 16/16, sandbox 11/11, and `edgion-dsl` compiler-loop 7/7 passed.
- Gateway all-target check, format, diff, agent-doc, English-only, and Gateway no-tracing checks passed for the implementation.
- Reviewer: control-flow 16/16, full `edgion-dsl` 114/114, and Gateway DSL 330/330 passed; format and diff checks passed.
- Coordinator full workspace checks passed: format, locked workspace all-target check and Clippy, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration guards.
- Frozen integration manifest: 3,231 paths unchanged.
- Commit: `6daaa9e8` (`Preserve DSL loop counters on break`); no push.
