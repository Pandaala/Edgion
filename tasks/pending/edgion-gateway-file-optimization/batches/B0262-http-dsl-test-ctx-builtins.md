# B0262 — HTTP DSL Context-builtin Tests

## Scope

- Pipeline state: complete; target-only implementation, independent final review, and coordinator gates passed.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `6daaa9e8`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/ctx_builtins.rs`.
- Target blob: `c6d5e89210cc7bfc64b9549f6633c5c727c1a66b`.
- Read scope includes the full test file, every exercised context builtin and VM/session path, adjacent tests, and current review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, target-only test integrity.
- Existing tests cover present/missing values and mutation failures, but not the production `checked_value_opt_str` enforcement when `ctx.get` receives a runtime value longer than `max_string_len`. Add an exact four-byte-versus-three-byte `StringTooLong` regression.
- Rename and reword the response-phase test so it accurately claims only that response execution can read a pre-existing context value; its mock does not execute a request-stage write.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/tests/ctx_builtins.rs` only. Preserve all production/context/error-policy/access-log/integration behavior.
- Separate follow-ups cover the production string copy, inaccurate access-log examples, and new request-to-response integration coverage; none expands this batch.

## Implementation and Review

- Added the exact four-byte runtime context value versus three-byte string-limit regression and asserted `StringTooLong { len: 4, limit: 3 }`.
- Renamed and reworded the response-phase test to claim only that it reads a pre-existing value.
- Independent final review: LGTM with no Must Fix or Should Fix. It independently traced the production builtin/VM limit path and confirmed the renamed response-phase test retains its actual wiring coverage.

## Verification

- Implementer: target 7/7 and full Gateway DSL 331/331 passed; Gateway all-target check, format, diff, English-only, and Gateway no-tracing checks passed.
- Reviewer: target 7/7 and full Gateway DSL 331/331 passed; format and diff checks passed.
- Coordinator full workspace checks passed: format, locked workspace all-target check and Clippy, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration guards.
- Frozen integration manifest: 3,231 paths unchanged.
- Commit: `82e00e04` (`Strengthen DSL context builtin tests`); no push.
