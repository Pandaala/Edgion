# Revalidate the DSL Compilation Trust-boundary ADR

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | docs |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Revalidate the DSL design-decision record against the current source-or-bytecode configuration flow and independent Controller/Gateway validation boundaries.

## Requirements

- Review `skills/04-review/dsl/intentional-design/dsl-design-decisions.md` against the current `DslConfig` schema, serialization, Controller plugin validation, config sync, Gateway plugin construction, `edgion_dsl::resolve_script`, bytecode validation, and `Vm::new` lifecycle.
- Correct the obsolete claims that the Controller is the sole compilation trust boundary, that the Gateway never parses source, and that only bytecode crosses config sync.
- Preserve the intentional separation between runtime-independent syntax/static validation and Gateway-owned VM/builtin initialization.
- Clearly distinguish observed current behavior, mandatory security boundaries, and any still-open architectural decision; do not preserve historical claims as current guarantees.
- Cross-check related DSL architecture/security pages and update only directly necessary backlinks or duplicated statements.
- Do not change runtime behavior, schemas, transport, logging, metrics, or integration tests as part of this documentation task.

## Scope

### In scope

- The DSL design-decision record and directly conflicting maintained DSL knowledge pages.

### Out of scope

- Runtime/compiler refactors, source removal, bytecode transport redesign, compatibility shims, and integration tests.

## Acceptance criteria

- The ADR accurately describes which representations may be configured and transported, where source/bytecode resolution occurs, and which validations each binary performs.
- All maintained DSL pages agree on the Controller/Gateway trust boundaries.
- Documentation validation and all applicable repository guards pass.
