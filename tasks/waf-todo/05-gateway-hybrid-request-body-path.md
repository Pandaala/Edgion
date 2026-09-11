# Integrate the Gateway Hybrid Request-Body Path

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | `04-transaction-hybrid-phase2.md` |

## Goal

Drive the transaction's required Phase 2 lanes from Pingora's post-mutation
request chunks while preserving Edgion's canonical request-body ownership and
retry behavior.

## Work

- Resolve the generation's lane plan before installing body handlers.
- Keep raw-only and structured-only requests out of `BodyStorage`.
- Use the canonical request `BodyStorage` only when `BoundedSnapshot` is
  required; do not add another request cache.
- Add explicit, independently configurable structured and compatibility
  retained-byte limits to the existing request-body policy resolution path.
- Resolve all sizes before the request hot path and copy numeric limits into
  `EdgionHttpContext.waf`.
- Preserve post-mutation ordering, monotonic retry disablement, fail-close
  abandoned behavior, and termination before forwarding a proven triggering
  chunk.
- Record one Phase 2 execution and bounded body-byte/memory observations per
  request.

## Tests

- Prove raw-only and JSON-only generations never populate `BodyStorage`.
- Cover H1 chunked, H2 data frames, Content-Length present/absent/incorrect,
  empty body, client abort, and early upstream response races.
- Verify Gateway physical cap versus engine, structured, and snapshot limits.
- Verify old/new generations remain pinned across a hot update during body
  streaming.

## Exit gate

Gateway Phase 2 is streaming-first for raw and JSON rules, uses only the
canonical bounded compatibility snapshot when required, and passes lifecycle,
cap, retry, and hot-reload tests.
