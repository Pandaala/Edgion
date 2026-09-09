# B0254 — HTTP DSL Module Facade

## Scope

- Pipeline state: in-progress; refreshed analysis passed the value gate after B0253.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `63e974bc`.
- Target: `edgion-gateway/src/plugins/http/dsl/mod.rs`.
- Target blob: `08c28c1371fb7ae25731decb5ee0c412090d9942`.
- Read scope includes the complete DSL module, current resource/compiler/VM ownership, public consumers, tests, architecture, and review precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, target-only rustdoc correction.
- The module header incorrectly says scripts are compiled on the Controller even though source/bytecode resolution and static validation run in both resource admission and Gateway construction. It also still describes `lang` as re-exporting the whole language core after B0252 removed the unused AST/compiler/parser aliases.
- The concise security layers omit independent Gateway bytecode revalidation and bounded runtime collection/output budgets, including B0253's bounded regex construction.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/mod.rs` only. Preserve declarations, visibility, runtime/schema/protocol/config/error-policy behavior, canonical shared-helper ownership, observability, and integrations.
- No behavioral test is justified; targeted Gateway all-target compile plus campaign gates are required.
- Separate `dsl/runtime.rs` construction-time wording stays with its later ledger batch. D0129/D0130 already own the confirmed body-read and sandbox documentation work.

## Implementation and Review

- Corrected only the module-level rustdoc: shared source/bytecode resolution and static validation, Gateway construction and execution ownership, the narrowed B0252 facade, Gateway bytecode defense, and bounded runtime collection/output layers.
- Preserved every declaration and visibility plus all runtime, schema, protocol, configuration, error-policy, observability, and integration behavior.
- Independent final review: LGTM, no Must Fix. The reviewer traced resource/Controller validation, shared resolution, Gateway plugin/VM construction, B0252 exports, and B0253 bounded replacement.
- The upcoming `dsl/runtime.rs` batch must include the directly related `dsl-engine.md` construction-time, `rate_limits`, and body-scanner path corrections in its frozen support scope.
- D0131 separately owns revalidation of the wider stale DSL compilation trust-boundary ADR.

## Verification

- Implementer and reviewer: Gateway locked all-target compile, format check, and diff check passed.
- Coordinator: `cargo fmt --all`; locked workspace check; locked workspace Clippy (exit 0 with pre-existing warnings); agent docs; SSA force; metrics inventory; Pingora source policy; Gateway no-tracing; English-only; diff check; and frozen paths all passed.
- Frozen integration manifest: 3,231 paths unchanged.
- Commit: `53b436aa` (`Correct DSL module ownership docs`); no push.
