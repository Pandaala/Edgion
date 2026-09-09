# B0259 — HTTP DSL Bytecode-deserialization Tests

## Scope

- Pipeline state: in-progress; refreshed analysis passed the value gate after B0258.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `9610ead5`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/bytecode_deserialization.rs`.
- Target blob: `a184896163d3cbf79f8a65d8344c9dc7157840c0`.
- Read scope includes the full test file, bytecode schema/deserialization/validation/VM construction, resource/Gateway trust boundaries, adjacent tests, and review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, target-only test integrity.
- Four hostile constant-pool validator fixtures still hardcode bytecode version 1 while the current `BYTECODE_VERSION` is 4, so their intended wrong-kind assertions are polluted by an unrelated unsupported-version error.
- Use `BYTECODE_VERSION` consistently for valid hand-crafted fixtures, derive a distinct unsupported version, remove the irrelevant old version field from truncated JSON, and correct pre-extraction/version comments while preserving all 13 branch-specific tests.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/tests/bytecode_deserialization.rs` only. Preserve all production/schema/protocol/error behavior and integrations. D0131 owns wider trust-boundary knowledge.

## Implementation and Review

- Replaced stale hardcoded versions in valid hostile fixtures with `BYTECODE_VERSION`, derived an overflow-safe distinct unsupported version, removed the irrelevant version member from truncated JSON, and corrected obsolete ownership/version comments.
- Preserved every branch-specific security test and assertion.
- Independent final review: LGTM, no Must Fix, Should Fix, or new deferred finding. It confirmed all valid fixtures are current-version, truncated JSON fails before version validation, and the 13 distinct branches remain intact.

## Verification

- Target tests: 13/13; full DSL filters: 329/329 by implementer and 361/361 by reviewer; Gateway locked all-target compile; format and diff checks passed.
- Coordinator: `cargo fmt --all`; locked workspace check; locked workspace Clippy (exit 0 with pre-existing warnings); agent docs; SSA force; metrics inventory; Pingora source policy; Gateway no-tracing; English-only; diff check; and frozen paths all passed.
- Frozen integration manifest: 3,231 paths unchanged.
- Commit: `d795f6e0` (`Keep DSL bytecode fixtures current`); no push.
