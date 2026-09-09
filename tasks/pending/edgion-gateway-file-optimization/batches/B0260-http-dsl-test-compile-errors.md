# B0260 — HTTP DSL Compile-error Tests

## Scope

- Pipeline state: complete; refreshed analysis passed the value gate after B0259.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `d795f6e0`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/compile_errors.rs`.
- Target blob: `bb8a0bf6312385e58cc749710c84d1813f096675`.
- Read scope includes the full test file, every exercised parser/compiler/validator error path, Gateway construction mapping, adjacent tests, and current review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, target-only test integrity.
- The invalid-regex test uses nonexistent infix syntax and fails in the parser before regex compilation. The instruction-limit test produces only about 601 opcodes but exceeds unrelated local/constant limits. Their `is_err()` helper hides both false positives.
- Make the helper assert an expected diagnostic fragment, pin all seven cases, use valid `.matches("[unclosed")` syntax, and use 501 repeated `true` statements to emit exactly 1,003 opcodes while staying below all competing limits.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/tests/compile_errors.rs` only. Preserve all production/parser/compiler/validator/error/integration behavior and create no issue.

## Implementation and Review

- Replaced the broad `is_err()` helper with expected-diagnostic matching and pinned all seven cases.
- Corrected invalid-regex input to valid method syntax and constructed exactly 1,003 instructions with 501 literal expression statements while staying below all competing limits.
- Independent final review: LGTM, no Must Fix, Should Fix, or deferred finding. It traced both corrected cases to their exact compiler/validator branches and verified all diagnostic fragments.

## Verification

- Target tests: 7/7; full DSL tests: 329/329; Gateway locked all-target compile; format and diff checks passed.
- Coordinator full workspace checks passed: format, locked workspace all-target check and Clippy, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration guards.
- Frozen integration manifest: 3,231 paths unchanged.
- Commit: `87fb44df` (`Strengthen DSL compile error tests`); no push.
