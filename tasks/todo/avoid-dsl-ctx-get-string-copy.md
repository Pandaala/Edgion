# Avoid the DSL `ctx.get` String Copy

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | perf |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Remove the avoidable request-path `String` copy between the shared HTTP context map and DSL `ctx.get` without weakening ownership, safety, or mocking boundaries.

## Requirements

- Trace `PluginSession::get_ctx_var`, `PingoraSessionAdapter`, `EdgionHttpContext::ctx_map`, DSL VM conversion, every implementation, and every consumer before selecting an ownership shape.
- Keep the production request path primary; do not retain an owned copy merely for `MockPluginSession` convenience.
- Preserve missing-value semantics and the UTF-8 byte-based `maxStringLen` check before exposing a DSL string.
- Reuse an existing borrowed or shared ownership form when it satisfies all lifetimes; do not add a second context store or per-request cache.
- Preserve context mutation, access-log projection redaction/blocklists, plugin-stage lifecycle, and object safety.
- Add ordinary unit coverage for the pure ownership/conversion boundary; keep mock coverage focused on wiring.
- Do not modify existing `edgion-tests/**` files.

## Scope

### In scope

- The canonical context store accessor, plugin-session trait/implementations, DSL `ctx.get` conversion, and directly affected unit tests.

### Out of scope

- Context schema changes, access-log policy, new caches, unrelated builtins, and integration tests.

## Acceptance criteria

- Production `ctx.get` no longer copies the stored string solely to return it through the session abstraction.
- Missing values and exact/over-limit UTF-8 byte behavior remain unchanged.
- All session implementations and consumers compile with the same semantic contract.
- Focused tests and all repository guards pass.
