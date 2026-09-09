# Avoid DSL Redis Link-key Clones

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Remove the avoidable namespace/name allocation performed by every DSL Redis builtin call while retaining synchronous generation-safe lookup and the canonical LinkSys Redis client store.

## Requirements

- Measure or instrument the current `resolve_redis_client` lookup path and confirm the namespace/name clone is a real per-call allocation.
- Borrow the immutable resolved link key through the synchronous `get_redis_client` lookup, or use an existing clone-cheap canonical key representation if borrowing cannot satisfy lifetimes.
- Do not retain a borrowed reference across an await, pin stale clients across runtime generations, add a second Redis store/cache, or weaken missing/not-ready behavior.
- Preserve alias resolution, LinkSys ownership, Redis builtin result shapes, error policy, and all command semantics.
- Add a focused regression or allocation assertion that distinguishes the old per-call deep clone from the new path without adding production instrumentation.

## Scope

### In scope

- DSL Redis link alias resolution and the narrow store lookup API needed to remove the confirmed allocation.

### Out of scope

- Redis error redaction, command batching, connection pooling, cache redesign, or request-metadata projection.

## Acceptance criteria

- A warm DSL Redis client lookup performs no deep namespace/name clone before command dispatch.
- Hot-reload generation and missing/not-ready semantics remain unchanged.
- Focused DSL/Redis tests and all repository guards pass.
