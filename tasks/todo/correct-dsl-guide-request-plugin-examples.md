# Correct DSL Guide Request-plugin Examples

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | docs |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Audit the bilingual DSL guides for malformed `requestPlugins` examples and make every YAML sample conform to the list-shaped plugin schema.

## Requirements

- Inspect all English and Chinese DSL user-guide examples that define `requestPlugins` or `responsePlugins`.
- Correct mappings that omit the list marker before `type: Dsl` or otherwise do not match the current EdgionPlugins schema.
- Keep the English and Chinese examples semantically equivalent while preserving intentionally different explanatory prose.
- Validate copied snippets against the current resource shape and plugin-stage rules; do not forward-port legacy keys.
- Do not change runtime behavior, schemas, integration tests, or unrelated documentation.

## Scope

### In scope

- DSL user-guide YAML examples and directly necessary bilingual cross-links.

### Out of scope

- DSL runtime/config changes, schema redesign, non-DSL plugin documentation, and integration tests.

## Acceptance criteria

- Every audited DSL plugin-list example is structurally valid YAML for the current schema.
- English and Chinese examples express the same configuration.
- Documentation validation and applicable repository guards pass.
