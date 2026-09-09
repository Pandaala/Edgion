# Enforce Exactly One CtxSet Transform Operation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | correctness |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Make the shared CtxSet configuration contract reject more than one transform operation instead of silently relying on Gateway priority.

## Requirements

- Count configured `replace`, `substring`, `case`, `prefix`, `suffix`, and enabled `trim` operations during shared resource validation.
- Accept zero or one operation and reject two or more with a stable, operator-facing validation error.
- Keep the Gateway's current operation priority as defensive behavior for any configuration that bypasses validation.
- Align schema-facing field documentation and examples with the enforced contract.
- Add focused shared resource and plugin-compilation tests for every valid single operation plus representative multi-operation rejection.
- Do not add compatibility aliases or modify existing integration tests.

## Acceptance criteria

- Controller and Gateway validation reject a CtxSet rule with multiple transform operations before runtime publication.
- Valid zero-operation and each single-operation configuration retain current behavior.
- Resource tests, plugin compilation tests, workspace checks, and repository guards pass.
