# B0250 — HTTP DSL Config

## Scope

- Pipeline state: changed; committed locally.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `f326f2800c03c34fce39f9fbedf73f18cacc2dc1`.
- Target: `edgion-gateway/src/plugins/http/dsl/config.rs`.
- Target blob: `5b293531643c8e197adc9e089aa6ecc0b3eb3887`.
- Read scope includes the complete Gateway DSL module, shared DSL compiler/configuration boundary, plugin runtime and direct tests/consumers.
- Exact write allow-list: `edgion-gateway/src/plugins/http/dsl/config.rs` only.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple documentation-only correction.
- The facade correctly and costlessly re-exports `DslConfig`, but its comment says the schema layer carries no engine dependency. Current architecture deliberately has `edgion-resources` depend on runtime-independent `edgion-dsl` for shared compilation/static validation; only Gateway VM, builtin dispatch and runtime integration remain Gateway-owned.
- Retain the re-export, visibility, API and double-validation defense unchanged. Correct only the three-layer ownership description; no test or integration issue is warranted.
- Decisive knowledge: Gateway DSL engine architecture, DSL coding guidance, shared compiler/validator boundaries, accepted double validation, response streaming/body admission and unit-testing rules.

## Outcome

- Corrected the module rustdoc to describe `edgion-resources` schema/static wrapper ownership, runtime-independent `edgion-dsl` parser/compiler/validator ownership, and Gateway `DslPlugin`/VM/builtin/runtime ownership.
- The `DslConfig` re-export, visibility, API, schema, double validation and runtime behavior are unchanged. Current target blob: `eed65840462e89ccaa369140506590b35f041ebc`.
- Fresh independent review returned LGTM with no Must Fix, Should Fix or deferred findings after verifying Cargo edges and compile/validation/runtime call chains.
- No unit or integration test is justified for a rustdoc-only correction. Workspace format/check/Clippy plus agent-doc, SSA, metrics, Pingora, Gateway no-tracing, English-only, diff and integration-freeze gates passed.
- Commit: `2123fbe7`; never pushed.
