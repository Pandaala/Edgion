# B0269 — HTTP DSL Bytecode Property Tests

## Scope

- Pipeline state: terminal; changed, independently reviewed and verified.
- Active anchor: `edgion-gateway`; baseline HEAD `12cfc41d`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/property_bytecode.rs`.
- Read-only investigation may follow the containing DSL module, compiler/validator/VM consumers, and relevant original knowledge.
- No technical writes authorized before the analysis/value gate. Existing integration tests and user-owned `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` are protected.

## Analysis

- Analyst: `/root/b0269_bytecode_analysis`; decision: change, simple.
- Valid-by-construction source templates were filtered with `prop_assume!` on compile errors, hiding regressions; VM initialization results and source-map lookup results were discarded.
- Exact technical allow-list: the target file only. Require compilation and VM initialization success; compare source-map lookup against bounded expanded RLE at every valid PC, a random PC, and both out-of-range boundaries.
- Preserve arbitrary-byte deserialize robustness; correct the fixed-regex generator's excessive ReDoS-coverage claim. No production or integration changes or deferred finding.
- Knowledge: root rules, DSL engine/security model, DSL coding/review indexes and regex DFA precedent, review routing, and unit-testing rules.

## Implementation and Independent Review

- Implementer `/root/b0269_implementation` changed only the approved test file; all four properties remain, with stronger valid-source and result checks and accurate coverage comments.
- Fresh reviewer `/root/b0269_independent_review`: PASS, no Must Fix or Should Fix. Verified valid-template assumptions, independent expanded-RLE oracle, boundary coverage, retained arbitrary-byte robustness and applicable red rules.

## Verification

- Focused properties: 4/4 passed (47-second build, 0.15-second execution).
- Formatting, agent-doc, SSA, metrics, Pingora source, no-tracing, English-only, diff and frozen integration checks passed. Locked workspace check and Clippy passed; 982 Gateway lib-test Clippy warnings remain.

## Handoff

- Ledger/changelog updated. Commit: pending automatic local commit; never push.
