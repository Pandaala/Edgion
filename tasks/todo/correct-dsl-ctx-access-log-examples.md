# Correct DSL Context Access-log Examples

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | docs |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Correct DSL examples that imply `ctx.set` values automatically enter the access log, and document the explicit, redacted `AccessLogExtern` projection boundary.

## Requirements

- Audit maintained English and Chinese DSL examples plus directly related plugin/access-log documentation for automatic-projection claims.
- State that `ctx.set` writes request context only; access-log output requires an explicit `AccessLogExtern` entry whose source is `ctx`.
- Preserve categorical blocked context keys, masking defaults, sensitive-value handling, and canonical access-log ownership.
- Keep English and Chinese user documentation semantically aligned.
- Do not change runtime behavior, schemas, defaults, logging, metrics, or integration tests.

## Scope

### In scope

- Directly inaccurate DSL and access-log documentation and necessary maintained-knowledge backlinks.

### Out of scope

- Plugin implementation, configuration schema changes, new projection modes, and test rewrites.

## Acceptance criteria

- No maintained example claims that arbitrary DSL context values are automatically logged.
- Examples show or link the explicit `AccessLogExtern` configuration and its masking/blocklist constraints.
- English and Chinese documentation agree.
- Documentation validation and applicable repository guards pass.
