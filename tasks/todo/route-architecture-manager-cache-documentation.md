# Route Architecture Manager-Cache Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | docs |
| Priority | P3 |
| Issue | N/A |

## AI guide summary

Align route architecture knowledge with the current request path, where HTTP and gRPC proxies retain listener-keyed route-manager handles instead of dynamically resolving a manager on every request.

## Requirements

- Revalidate HTTP, gRPC, TCP, UDP, and TLS manager ownership against current code.
- Correct `route-architecture-consistency.md` statements that still describe dynamic HTTP/gRPC manager lookup.
- Preserve historical rationale while clearly distinguishing the current cached-handle design from legacy behavior.
- Cross-check retained-manager removal and recreation semantics documented by the route-manager cleanup campaign.

## Scope

### In scope

- Routing review knowledge about manager lookup, cache ownership, and request-time access.
- Links to the current HTTP/gRPC proxy fields and manager invalidation behavior.

### Out of scope

- Runtime changes to manager publication, lookup, or invalidation.
- B0176's per-listener proxy-state cleanup.
- Changes to existing `edgion-tests/**` files.

## Affected modules

| Module | Impact |
|--------|--------|
| `skills/04-review/routing/intentional-design/route-architecture-consistency.md` | Replace obsolete dynamic-resolution claims with current cached-handle ownership. |
| HTTP/gRPC route managers | Source evidence only; no runtime change. |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Defer from B0176 | The stale statement is independent documentation debt and should not expand a Pingora lifecycle cleanup. |
